//! The Verisure web API (« OWA », GraphQL over HTTPS), as the mobile app
//! speaks it. Nothing here is published by Verisure: the shapes come from
//! the open-source clients that reverse-engineered it (notably
//! `guerrerotook/securitas-direct-new-api`, Apache-2.0).
//!
//! Three rules shape this file:
//! - a wrong password counts toward a lock of the account: a login is never
//!   repeated by this module (the caller decides, once);
//! - the refresh token changes at every use: the caller stores each new one
//!   before doing anything else;
//! - requests are spaced, and a block by the firewall is reported, never
//!   retried here.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use http::{HeaderName, HeaderValue};
use http_body_util::Full;
use moli_net::Body;
use serde_json::{Value as Json, json};

use crate::model::{self, Installation, Phone};

const CALLBY: &str = "OWA_10";
const APP_VERSION: &str = "10.102.0";
/// The service whose status is the alarm's.
const ALARM_SERVICE: &str = "11";
const PATH: &str = "/owa-api/graphql";
/// Two requests are never closer than this (the firewall counts them).
const MIN_GAP: Duration = Duration::from_secs(2);
const REQUEST_LIMIT: Duration = Duration::from_secs(25);
const MAX_ANSWER: usize = 2 * 1024 * 1024;
const POLL_DELAY: Duration = Duration::from_secs(2);
const POLL_BUDGET: Duration = Duration::from_secs(60);
/// A session token is renewed this long before it expires.
const RENEW_BEFORE: u64 = 90;
/// The capabilities token is read again this often.
const CAPABILITIES_TTL: Duration = Duration::from_secs(20 * 60);

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.124 Safari/537.36 Edg/102.0.1245.41";

const LOGIN: &str = "mutation mkLoginToken($user: String!, $password: String!, $id: String!, $country: String!, $lang: String!, $callby: String!, $idDevice: String!, $idDeviceIndigitall: String!, $deviceType: String!, $deviceVersion: String!, $deviceResolution: String!, $deviceName: String!, $deviceBrand: String!, $deviceOsVersion: String!, $uuid: String!) { xSLoginToken(user: $user, password: $password, country: $country, lang: $lang, callby: $callby, id: $id, idDevice: $idDevice, idDeviceIndigitall: $idDeviceIndigitall, deviceType: $deviceType, deviceVersion: $deviceVersion, deviceResolution: $deviceResolution, deviceName: $deviceName, deviceBrand: $deviceBrand, deviceOsVersion: $deviceOsVersion, uuid: $uuid) { __typename res msg hash refreshToken legals changePassword needDeviceAuthorization mainUser } }";

const REFRESH: &str = "mutation RefreshLogin($refreshToken: String!, $id: String!, $country: String!, $lang: String!, $callby: String!, $idDevice: String!, $idDeviceIndigitall: String!, $deviceType: String!, $deviceVersion: String!, $deviceResolution: String!, $deviceName: String!, $deviceBrand: String!, $deviceOsVersion: String!, $uuid: String!) {\n  xSRefreshLogin(refreshToken: $refreshToken, id: $id, country: $country, lang: $lang, callby: $callby, idDevice: $idDevice, idDeviceIndigitall: $idDeviceIndigitall, deviceType: $deviceType, deviceVersion: $deviceVersion, deviceResolution: $deviceResolution, deviceName: $deviceName, deviceBrand: $deviceBrand, deviceOsVersion: $deviceOsVersion, uuid: $uuid) {\n    __typename\n    res\n    msg\n    hash\n    refreshToken\n    legals\n    changePassword\n    needDeviceAuthorization\n    mainUser\n  }\n}";

const VALIDATE_DEVICE: &str = "mutation mkValidateDevice($idDevice: String, $idDeviceIndigitall: String, $uuid: String, $deviceName: String, $deviceBrand: String, $deviceOsVersion: String, $deviceVersion: String) {\n  xSValidateDevice(idDevice: $idDevice, idDeviceIndigitall: $idDeviceIndigitall, uuid: $uuid, deviceName: $deviceName, deviceBrand: $deviceBrand, deviceOsVersion: $deviceOsVersion, deviceVersion: $deviceVersion) {\n    res\n    msg\n    hash\n    refreshToken\n    legals\n  }\n}\n";

const SEND_OTP: &str = "mutation mkSendOTP($recordId: Int!, $otpHash: String!) {\n  xSSendOtp(recordId: $recordId, otpHash: $otpHash) {\n    res\n    msg\n  }\n}\n";

const INSTALLATIONS: &str = "query mkInstallationList {\n  xSInstallations {\n    installations {\n      numinst\n      alias\n      panel\n      type\n      name\n      surname\n      address\n      city\n      postcode\n      province\n      email\n      phone\n    }\n  }\n}\n";

const SERVICES: &str = "query Srv($numinst: String!, $uuid: String) {\n  xSSrv(numinst: $numinst, uuid: $uuid) {\n    res\n    msg\n    language\n    installation {\n      numinst\n      role\n      alias\n      status\n      panel\n      sim\n      instIbs\n      services {\n        idService\n        active\n        visible\n        bde\n        isPremium\n        codOper\n        request\n        minWrapperVersion\n        unprotectActive\n        unprotectDeviceStatus\n        instDate\n        genericConfig {\n          total\n          attributes {\n            key\n            value\n          }\n        }\n        attributes {\n          attributes {\n            name\n            value\n            active\n          }\n        }\n      }\n      configRepoUser {\n        alarmPartitions {\n          id\n          enterStates\n          leaveStates\n        }\n      }\n      capabilities\n    }\n  }\n}";

const CHECK_ALARM: &str = "query CheckAlarm($numinst: String!, $panel: String!) {\n  xSCheckAlarm(numinst: $numinst, panel: $panel) {\n    res\n    msg\n    referenceId\n  }\n}\n";

const CHECK_ALARM_STATUS: &str = "query CheckAlarmStatus($numinst: String!, $idService: String!, $panel: String!, $referenceId: String!) {\n  xSCheckAlarmStatus(numinst: $numinst, idService: $idService, panel: $panel, referenceId: $referenceId) {\n    res\n    msg\n    status\n    numinst\n    protomResponse\n    protomResponseDate\n  }\n}\n";

const ARM_PANEL: &str = "mutation xSArmPanel($numinst: String!, $request: ArmCodeRequest!, $panel: String!, $currentStatus: String, $suid: String, $forceArmingRemoteId: String, $armAndLock: Boolean) {\n  xSArmPanel(numinst: $numinst, request: $request, panel: $panel, currentStatus: $currentStatus, suid: $suid, forceArmingRemoteId: $forceArmingRemoteId, armAndLock: $armAndLock) {\n    res\n    msg\n    referenceId\n  }\n}\n";

const ARM_STATUS: &str = "query ArmStatus($numinst: String!, $request: ArmCodeRequest, $panel: String!, $referenceId: String!, $counter: Int!, $forceArmingRemoteId: String, $armAndLock: Boolean) {\n  xSArmStatus(numinst: $numinst, panel: $panel, referenceId: $referenceId, counter: $counter, request: $request, forceArmingRemoteId: $forceArmingRemoteId, armAndLock: $armAndLock) {\n    res\n    msg\n    status\n    protomResponse\n    protomResponseDate\n    numinst\n    requestId\n    error {\n      code\n      type\n      allowForcing\n      exceptionsNumber\n      referenceId\n      suid\n    }\n  }\n}\n";

const DISARM_PANEL: &str = "mutation xSDisarmPanel($numinst: String!, $request: DisarmCodeRequest!, $panel: String!) {\n  xSDisarmPanel(numinst: $numinst, request: $request, panel: $panel) {\n    res\n    msg\n    referenceId\n  }\n}\n";

const DISARM_STATUS: &str = "query DisarmStatus($numinst: String!, $panel: String!, $referenceId: String!, $counter: Int!, $request: DisarmCodeRequest) {\n  xSDisarmStatus(numinst: $numinst, panel: $panel, referenceId: $referenceId, counter: $counter, request: $request) {\n    res\n    msg\n    status\n    protomResponse\n    protomResponseDate\n    numinst\n    requestId\n    error {\n      code\n      type\n      allowForcing\n      exceptionsNumber\n      referenceId\n    }\n  }\n}\n";

/// Why a call did not go through.
#[derive(Debug)]
pub enum ApiError {
    /// The firewall blocked us: back off for a long while.
    Waf,
    /// Too many requests (HTTP 403 without the firewall's page).
    RateLimited,
    /// The session token is no longer good: renew it.
    SessionExpired,
    /// The account is locked (too many wrong passwords): a human acts.
    Blocked,
    /// Verisure said no (credentials, request); the text says why.
    Refused(String),
    /// The panel did not answer in time.
    Timeout,
    /// The network, or an answer that is no JSON.
    Network(anyhow::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Waf => f.write_str("verisure firewall block"),
            Self::RateLimited => f.write_str("verisure rate limit"),
            Self::SessionExpired => f.write_str("verisure session expired"),
            Self::Blocked => f.write_str("verisure account locked"),
            Self::Refused(why) => write!(f, "verisure refused: {why}"),
            Self::Timeout => f.write_str("the panel did not answer in time"),
            Self::Network(e) => write!(f, "verisure unreachable: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::Network(e)
    }
}

/// The tokens a login or a refresh hands over.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub hash: String,
    pub refresh: Option<String>,
}

/// What a login said.
#[derive(Debug)]
pub enum LoginOutcome {
    Tokens(Tokens),
    /// A new device: a second factor (SMS) is required.
    NeedsDevice,
}

/// A device validation: tokens, or the challenge to answer.
#[derive(Debug)]
pub enum DeviceStep {
    Tokens(Tokens),
    Challenge { hash: String, phones: Vec<Phone> },
}

/// How a command ended on the panel.
#[derive(Debug)]
pub struct CommandResult {
    /// The panel's code after the command.
    pub code: Option<String>,
    /// Why the panel refused, when it did.
    pub refusal: Option<String>,
}

enum Auth {
    /// Credentials in the variables, nothing in the headers.
    Login,
    /// Empty token in the header (refresh, device validation).
    Bare,
    /// The session token.
    Session,
}

pub struct Client {
    host: String,
    country: String,
    lang: String,
    username: String,
    device: String,
    operation_id: String,
    hash: Option<String>,
    expires: u64,
    login_ts: u64,
    capabilities: Option<(String, Instant)>,
    otp: Option<(String, String)>,
    last_request: Option<Instant>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never a token, a code or a name: only what helps to read a log.
        f.debug_struct("Client")
            .field("host", &self.host)
            .field("has_session", &self.hash.is_some())
            .finish_non_exhaustive()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

/// The API's domain for a country.
#[must_use]
pub fn host_for(country: &str) -> String {
    match country.to_ascii_uppercase().as_str() {
        "AR" => "customers.verisure.com.ar".into(),
        "BR" => "customers.verisure.com.br".into(),
        "CL" => "customers.verisure.cl".into(),
        "ES" => "customers.verisure.es".into(),
        "GB" => "customers.verisure.co.uk".into(),
        "IE" => "customers.verisure.ie".into(),
        "IT" => "customers.verisure.it".into(),
        "PE" => "customers.verisure.pe".into(),
        "PT" => "customers.verisure.pt".into(),
        other => format!("customers.securitasdirect.{}", other.to_ascii_lowercase()),
    }
}

/// The language the app uses for a country.
#[must_use]
pub fn language_for(country: &str) -> &'static str {
    match country.to_ascii_uppercase().as_str() {
        "BR" => "br",
        "CL" | "ES" | "PE" => "es",
        "FR" => "fr",
        "IT" => "it",
        "AR" => "ar",
        "PT" => "pt",
        _ => "en",
    }
}

/// `n` random hexadecimal characters: names for this installation of Moli,
/// not secrets.
#[must_use]
pub fn random_hex(n: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let mut out = String::with_capacity(n + 16);
    let mut counter = 0u64;
    while out.len() < n {
        let mut hasher = state.build_hasher();
        hasher.write_u64(counter);
        hasher.write_u64(now_millis());
        out.push_str(&format!("{:016x}", hasher.finish()));
        counter += 1;
    }
    out.truncate(n);
    out
}

impl Client {
    #[must_use]
    pub fn new(country: &str, username: &str, device: &str) -> Self {
        Self {
            host: host_for(country),
            country: country.to_ascii_uppercase(),
            lang: language_for(country).to_owned(),
            username: username.to_owned(),
            device: device.to_owned(),
            operation_id: random_hex(128),
            hash: None,
            expires: 0,
            login_ts: 0,
            capabilities: None,
            otp: None,
            last_request: None,
        }
    }

    /// Whether a session token is held and good for a little while.
    #[must_use]
    pub fn has_session(&self) -> bool {
        self.hash.is_some() && now_secs() + RENEW_BEFORE < self.expires
    }

    fn adopt(&mut self, tokens: &Tokens) {
        self.expires = model::jwt_exp(&tokens.hash).unwrap_or_else(|| now_secs() + 10 * 60);
        self.hash = Some(tokens.hash.clone());
        self.login_ts = now_millis();
    }

    fn request_id(&self) -> String {
        format!(
            "OWA_______________{}_______________{}",
            self.username,
            now_millis()
        )
    }

    fn device_variables(&self) -> serde_json::Map<String, Json> {
        let Json::Object(map) = json!({
            "idDevice": self.device,
            "idDeviceIndigitall": "",
            "uuid": self.device,
            "deviceType": "",
            "deviceVersion": APP_VERSION,
            "deviceResolution": "",
            "deviceName": "SM-S901U",
            "deviceBrand": "samsung",
            "deviceOsVersion": "12",
        }) else {
            unreachable!("a JSON object literal")
        };
        map
    }

    fn headers(
        &self,
        operation: &str,
        auth: &Auth,
        installation: Option<&Installation>,
    ) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "app".to_owned(),
                json!({"appVersion": APP_VERSION, "origin": "native"}).to_string(),
            ),
            ("User-Agent".to_owned(), USER_AGENT.to_owned()),
            ("X-APOLLO-OPERATION-ID".to_owned(), self.operation_id.clone()),
            ("X-APOLLO-OPERATION-NAME".to_owned(), operation.to_owned()),
            ("extension".to_owned(), r#"{"mode":"full"}"#.to_owned()),
            ("content-type".to_owned(), "application/json; charset=utf-8".to_owned()),
        ];
        if let Some(i) = installation {
            headers.push(("numinst".to_owned(), i.number.clone()));
            headers.push(("panel".to_owned(), i.panel.clone()));
            if let Some((token, _)) = &self.capabilities {
                headers.push(("X-Capabilities".to_owned(), token.clone()));
            }
        }
        let hash = match auth {
            Auth::Login => None,
            Auth::Bare => Some(json!({"hash": "", "refreshToken": ""})),
            Auth::Session => self.hash.as_ref().map(|h| json!({"hash": h})),
        };
        if let Some(Json::Object(mut extra)) = hash {
            extra.insert("loginTimestamp".into(), json!(self.login_ts));
            extra.insert("user".into(), json!(self.username));
            extra.insert("id".into(), json!(self.request_id()));
            extra.insert("country".into(), json!(self.country));
            extra.insert("lang".into(), json!(self.lang));
            extra.insert("callby".into(), json!(CALLBY));
            headers.push(("auth".to_owned(), Json::Object(extra).to_string()));
        }
        if let Some((hash, code)) = &self.otp {
            headers.push((
                "security".to_owned(),
                json!({"token": code, "type": "OTP", "otpHash": hash}).to_string(),
            ));
        }
        headers
    }

    /// One GraphQL call. The answer's JSON is returned whatever it says
    /// (login and device validation read their errors' data); only the
    /// transport, the firewall, the rate limit and an expired session are
    /// errors here.
    async fn call(
        &mut self,
        operation: &str,
        query: &str,
        variables: Json,
        auth: Auth,
        installation: Option<&Installation>,
    ) -> Result<Json, ApiError> {
        if let Some(last) = self.last_request {
            let wait = MIN_GAP.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let headers = self.headers(operation, &auth, installation);
        let body = json!({"operationName": operation, "variables": variables, "query": query});
        let mut request = http::Request::builder().method(http::Method::POST).uri(PATH);
        for (name, value) in &headers {
            request = request.header(
                HeaderName::from_bytes(name.as_bytes()).map_err(|e| anyhow!("{e}"))?,
                HeaderValue::from_bytes(value.as_bytes()).map_err(|e| anyhow!("{e}"))?,
            );
        }
        let request = request
            .body(Full::new(Body::from(body.to_string().into_bytes())))
            .map_err(|e| anyhow!("{e}"))?;
        self.last_request = Some(Instant::now());
        let (status, _, bytes) =
            moli_net::web_with_headers(&self.host, 443, true, request, REQUEST_LIMIT, MAX_ANSWER)
                .await?;
        let text = String::from_utf8_lossy(&bytes);
        if status == http::StatusCode::FORBIDDEN {
            return Err(if text.contains("_Incapsula_Resource") {
                ApiError::Waf
            } else {
                ApiError::RateLimited
            });
        }
        let answer: Json = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::Network(anyhow!("HTTP {status}, no JSON")))?;
        let errors = answer["errors"].to_string();
        if matches!(auth, Auth::Session)
            && (status == http::StatusCode::UNAUTHORIZED
                || errors.contains("60067")
                || errors.contains("Invalid Session"))
        {
            return Err(ApiError::SessionExpired);
        }
        if errors.contains("60052") {
            return Err(ApiError::Blocked);
        }
        Ok(answer)
    }

    // ── authentication ──────────────────────────────────────────────────

    /// One login. Never repeated here: the caller records that it was
    /// tried.
    pub async fn login(&mut self, password: &str) -> Result<LoginOutcome, ApiError> {
        let mut vars = self.device_variables();
        vars.insert("user".into(), json!(self.username));
        vars.insert("password".into(), json!(password));
        vars.insert("id".into(), json!(self.request_id()));
        vars.insert("country".into(), json!(self.country));
        vars.insert("lang".into(), json!(self.lang));
        vars.insert("callby".into(), json!(CALLBY));
        let answer = self
            .call("mkLoginToken", LOGIN, Json::Object(vars), Auth::Login, None)
            .await?;
        let data = &answer["data"]["xSLoginToken"];
        if data["needDeviceAuthorization"].as_bool() == Some(true) {
            return Ok(LoginOutcome::NeedsDevice);
        }
        match data["hash"].as_str().filter(|h| !h.is_empty()) {
            Some(hash) => {
                let tokens = Tokens {
                    hash: hash.to_owned(),
                    refresh: data["refreshToken"].as_str().map(str::to_owned),
                };
                self.adopt(&tokens);
                Ok(LoginOutcome::Tokens(tokens))
            }
            None => Err(ApiError::Refused(first_error(&answer))),
        }
    }

    /// Renews the session. `Ok(None)`: the token is no longer accepted.
    pub async fn refresh(&mut self, refresh_token: &str) -> Result<Option<Tokens>, ApiError> {
        let mut vars = self.device_variables();
        vars.insert("refreshToken".into(), json!(refresh_token));
        vars.insert("id".into(), json!(self.request_id()));
        vars.insert("country".into(), json!(self.country));
        vars.insert("lang".into(), json!(self.lang));
        vars.insert("callby".into(), json!(CALLBY));
        let answer = self
            .call("RefreshLogin", REFRESH, Json::Object(vars), Auth::Bare, None)
            .await?;
        let data = &answer["data"]["xSRefreshLogin"];
        if data["res"].as_str() != Some("OK") {
            return Ok(None);
        }
        let Some(hash) = data["hash"].as_str().filter(|h| !h.is_empty()) else {
            return Ok(None);
        };
        let tokens = Tokens {
            hash: hash.to_owned(),
            refresh: data["refreshToken"].as_str().map(str::to_owned),
        };
        self.adopt(&tokens);
        Ok(Some(tokens))
    }

    /// Asks to validate this device: with no code, Verisure answers with
    /// the challenge; with the SMS code, with tokens.
    pub async fn validate_device(
        &mut self,
        otp: Option<(&str, &str)>,
    ) -> Result<DeviceStep, ApiError> {
        self.otp = otp.map(|(hash, code)| (hash.to_owned(), code.to_owned()));
        let vars = json!({
            "idDevice": self.device,
            "idDeviceIndigitall": "",
            "uuid": self.device,
            "deviceName": "SM-S901U",
            "deviceBrand": "samsung",
            "deviceOsVersion": "12",
            "deviceVersion": APP_VERSION,
        });
        let result = self
            .call("mkValidateDevice", VALIDATE_DEVICE, vars, Auth::Bare, None)
            .await;
        // A code is good for one try: never kept for the next request.
        self.otp = None;
        let answer = result?;
        if let Some((hash, phones)) = answer["errors"]
            .get(0)
            .and_then(|e| model::challenge(&e["data"]))
        {
            return Ok(DeviceStep::Challenge { hash, phones });
        }
        let data = &answer["data"]["xSValidateDevice"];
        match data["hash"].as_str().filter(|h| !h.is_empty()) {
            Some(hash) => {
                let tokens = Tokens {
                    hash: hash.to_owned(),
                    refresh: data["refreshToken"].as_str().map(str::to_owned),
                };
                self.adopt(&tokens);
                Ok(DeviceStep::Tokens(tokens))
            }
            None => Err(ApiError::Refused(first_error(&answer))),
        }
    }

    /// Sends the SMS to one phone of the challenge.
    pub async fn send_otp(&mut self, phone: i64, hash: &str) -> Result<(), ApiError> {
        let answer = self
            .call(
                "mkSendOTP",
                SEND_OTP,
                json!({"recordId": phone, "otpHash": hash}),
                Auth::Bare,
                None,
            )
            .await?;
        if answer["data"]["xSSendOtp"]["res"].as_str() == Some("OK") {
            Ok(())
        } else {
            Err(ApiError::Refused(first_error(&answer)))
        }
    }

    // ── the installation ────────────────────────────────────────────────

    pub async fn installations(&mut self) -> Result<Vec<Installation>, ApiError> {
        let answer = self
            .call("mkInstallationList", INSTALLATIONS, json!({}), Auth::Session, None)
            .await?;
        Ok(model::installations(&answer))
    }

    /// Reads the capabilities token when it is missing or old.
    pub async fn ensure_capabilities(&mut self, installation: &Installation) -> Result<(), ApiError> {
        if self
            .capabilities
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < CAPABILITIES_TTL)
        {
            return Ok(());
        }
        let answer = self
            .call(
                "Srv",
                SERVICES,
                json!({"numinst": installation.number, "uuid": self.device}),
                Auth::Session,
                Some(installation),
            )
            .await?;
        if let Some(token) = answer["data"]["xSSrv"]["installation"]["capabilities"].as_str() {
            self.capabilities = Some((token.to_owned(), Instant::now()));
        }
        Ok(())
    }

    // ── the alarm ───────────────────────────────────────────────────────

    /// Waits for an operation's result: polls until the panel stops
    /// saying `WAIT`.
    async fn poll(
        &mut self,
        installation: &Installation,
        operation: &str,
        query: &str,
        field: &str,
        variables: impl Fn(u32) -> Json,
    ) -> Result<Json, ApiError> {
        let deadline = Instant::now() + POLL_BUDGET;
        let mut counter = 0u32;
        loop {
            counter += 1;
            let answer = self
                .call(operation, query, variables(counter), Auth::Session, Some(installation))
                .await?;
            let status = answer["data"][field].clone();
            if status.is_null() {
                return Err(ApiError::Refused(first_error(&answer)));
            }
            if status["res"].as_str() != Some("WAIT") {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(ApiError::Timeout);
            }
            tokio::time::sleep(POLL_DELAY).await;
        }
    }

    /// Asks the panel where it stands and returns its code.
    pub async fn check_alarm(&mut self, installation: &Installation) -> Result<String, ApiError> {
        self.ensure_capabilities(installation).await?;
        let vars = json!({"numinst": installation.number, "panel": installation.panel});
        let answer = self
            .call("CheckAlarm", CHECK_ALARM, vars, Auth::Session, Some(installation))
            .await?;
        let reference = answer["data"]["xSCheckAlarm"]["referenceId"]
            .as_str()
            .ok_or_else(|| ApiError::Refused(first_error(&answer)))?
            .to_owned();
        let number = installation.number.clone();
        let panel = installation.panel.clone();
        let status = self
            .poll(installation, "CheckAlarmStatus", CHECK_ALARM_STATUS, "xSCheckAlarmStatus", |_| {
                json!({
                    "numinst": number,
                    "panel": panel,
                    "referenceId": reference,
                    "idService": ALARM_SERVICE,
                })
            })
            .await?;
        if status["res"].as_str() != Some("OK") {
            return Err(ApiError::Refused(status["msg"].as_str().unwrap_or("check failed").to_owned()));
        }
        status["protomResponse"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Refused("no state in the answer".into()))
    }

    /// Sends an arm or disarm order; returns its reference. The order is
    /// then on its way to the panel: `finish_command` waits for the result.
    pub async fn begin_command(
        &mut self,
        installation: &Installation,
        command: &str,
        current: &str,
    ) -> Result<String, ApiError> {
        self.ensure_capabilities(installation).await?;
        let (operation, query, field, vars) = if model::is_disarm(command) {
            (
                "xSDisarmPanel",
                DISARM_PANEL,
                "xSDisarmPanel",
                json!({
                    "request": command,
                    "numinst": installation.number,
                    "panel": installation.panel,
                    "currentStatus": current,
                }),
            )
        } else {
            (
                "xSArmPanel",
                ARM_PANEL,
                "xSArmPanel",
                json!({
                    "request": command,
                    "numinst": installation.number,
                    "panel": installation.panel,
                    "currentStatus": current,
                    "armAndLock": false,
                }),
            )
        };
        let answer = self
            .call(operation, query, vars, Auth::Session, Some(installation))
            .await?;
        let submitted = &answer["data"][field];
        if submitted["res"].as_str() == Some("ERROR") {
            return Err(ApiError::Refused(
                submitted["msg"].as_str().unwrap_or("order refused").to_owned(),
            ));
        }
        submitted["referenceId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Refused(first_error(&answer)))
    }

    /// Waits for the panel's result of an order sent by `begin_command`.
    pub async fn finish_command(
        &mut self,
        installation: &Installation,
        command: &str,
        current: &str,
        reference: &str,
    ) -> Result<CommandResult, ApiError> {
        let (operation, query, field) = if model::is_disarm(command) {
            ("DisarmStatus", DISARM_STATUS, "xSDisarmStatus")
        } else {
            ("ArmStatus", ARM_STATUS, "xSArmStatus")
        };
        let number = installation.number.clone();
        let panel = installation.panel.clone();
        let disarm = model::is_disarm(command);
        let status = self
            .poll(installation, operation, query, field, |counter| {
                let mut vars = json!({
                    "request": command,
                    "numinst": number,
                    "panel": panel,
                    "referenceId": reference,
                    "counter": counter,
                });
                if disarm {
                    vars["currentStatus"] = json!(current);
                } else {
                    vars["armAndLock"] = json!(false);
                }
                vars
            })
            .await?;
        let code = status["protomResponse"].as_str().map(str::to_owned);
        let refusal = (status["res"].as_str() == Some("ERROR")).then(|| {
            let msg = status["msg"].as_str().unwrap_or("refused");
            if status["error"]["exceptionsNumber"]
                .as_str()
                .and_then(|n| n.parse::<u32>().ok())
                .or_else(|| status["error"]["exceptionsNumber"].as_u64().map(|n| n as u32))
                .is_some_and(|n| n > 0)
            {
                format!("{msg} (open zones)")
            } else {
                msg.to_owned()
            }
        });
        Ok(CommandResult { code, refusal })
    }
}

/// The first error message of an answer, for a person to read.
fn first_error(answer: &Json) -> String {
    answer["errors"]
        .get(0)
        .and_then(|e| e["message"].as_str())
        .or_else(|| {
            answer["data"]
                .as_object()
                .and_then(|d| d.values().find_map(|v| v["msg"].as_str()))
        })
        .unwrap_or("no reason given")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countries_have_their_domain_and_language() {
        assert_eq!(host_for("FR"), "customers.securitasdirect.fr");
        assert_eq!(host_for("fr"), "customers.securitasdirect.fr");
        assert_eq!(host_for("ES"), "customers.verisure.es");
        assert_eq!(host_for("GB"), "customers.verisure.co.uk");
        assert_eq!(language_for("FR"), "fr");
        assert_eq!(language_for("DE"), "en");
    }

    #[test]
    fn random_names_have_the_asked_length() {
        assert_eq!(random_hex(16).len(), 16);
        assert_eq!(random_hex(128).len(), 128);
        assert_ne!(random_hex(16), random_hex(16));
        assert!(random_hex(32).chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn headers_carry_the_session_but_never_in_a_login() {
        let mut client = Client::new("FR", "someone", "abcdef0123456789");
        let login = client.headers("mkLoginToken", &Auth::Login, None);
        assert!(login.iter().all(|(n, _)| n != "auth"));
        client.hash = Some("tok.en.x".into());
        let installation = Installation {
            number: "123".into(),
            alias: String::new(),
            panel: "SDVFAST".into(),
        };
        let h = client.headers("CheckAlarm", &Auth::Session, Some(&installation));
        let auth = h.iter().find(|(n, _)| n == "auth").unwrap();
        assert!(auth.1.contains(r#""hash":"tok.en.x""#));
        assert!(auth.1.contains(r#""country":"FR""#));
        assert!(h.iter().any(|(n, v)| n == "numinst" && v == "123"));
        assert!(h.iter().any(|(n, v)| n == "panel" && v == "SDVFAST"));
        // The empty-token form (refresh, device validation).
        let bare = client.headers("RefreshLogin", &Auth::Bare, None);
        let auth = bare.iter().find(|(n, _)| n == "auth").unwrap();
        assert!(auth.1.contains(r#""hash":"""#));
    }

    #[test]
    fn the_second_factor_goes_in_a_header_for_one_call() {
        let mut client = Client::new("FR", "someone", "abcdef0123456789");
        client.otp = Some(("h4sh".into(), "123456".into()));
        let h = client.headers("mkValidateDevice", &Auth::Bare, None);
        let security = h.iter().find(|(n, _)| n == "security").unwrap();
        assert!(security.1.contains(r#""otpHash":"h4sh""#));
        assert!(security.1.contains(r#""token":"123456""#));
    }

    #[test]
    fn a_debug_never_shows_a_token() {
        let mut client = Client::new("FR", "someone", "abcdef0123456789");
        client.hash = Some("secret-session-token".into());
        client.otp = Some(("hash".into(), "654321".into()));
        let shown = format!("{client:?}");
        assert!(!shown.contains("secret-session-token"));
        assert!(!shown.contains("654321"));
        assert!(!shown.contains("someone"));
    }

    #[test]
    fn the_first_error_is_found_where_it_is() {
        let at_top = serde_json::json!({"errors": [{"message": "Invalid credentials"}]});
        assert_eq!(first_error(&at_top), "Invalid credentials");
        let in_data = serde_json::json!({"data": {"xSArmPanel": {"res": "ERROR", "msg": "no way"}}});
        assert_eq!(first_error(&in_data), "no way");
        assert_eq!(first_error(&serde_json::json!({})), "no reason given");
    }
}
