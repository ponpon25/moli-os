//! The airCloud Go account API, as the mobile app speaks it (REST for the
//! account and the orders, a STOMP WebSocket for the units' state). Nothing
//! here is published by Hitachi: the shapes come from the app's own traffic.
//!
//! Rules: a refused password is reported, never retried here (the caller
//! parks); a session lasts 30 minutes and is renewed by signing in again,
//! which is what the app itself does; orders carry the unit's whole state.

use std::time::{Duration, Instant};

use anyhow::anyhow;
use http::{HeaderName, HeaderValue, Method};
use http_body_util::Full;
use moli_net::Body;
use moli_net::ws::WebSocket;
use serde_json::{Value as Json, json};

use crate::model::{Capabilities, Unit, order_body};
use crate::stomp::{self, Frame};

pub const API_HOST: &str = "api-global-prod.aircloudhome.com";
pub const NOTIFICATION_HOST: &str = "notification-global-prod.aircloudhome.com";
const SOCKET_PATH: &str = "/rac-notifications/websocket";
const USER_AGENT: &str = "airCloud%20Home/30 CFNetwork/3896.100.1.2.1 Darwin/27.0.0";
const REQUEST_LIMIT: Duration = Duration::from_secs(15);
/// An order waits for the unit's answer, which Hitachi's cloud takes its time over.
const ORDER_LIMIT: Duration = Duration::from_secs(40);
const MAX_ANSWER: usize = 1024 * 1024;
/// Two requests are never closer than this.
const MIN_GAP: Duration = Duration::from_millis(500);
/// The session is renewed this long before the token ends.
pub const RENEW_BEFORE: Duration = Duration::from_secs(3 * 60);
const DEFAULT_LIFETIME: Duration = Duration::from_secs(30 * 60);

/// Why a call did not go through.
#[derive(Debug)]
pub enum ApiError {
    /// The account refused the identifier or the password: a person acts.
    Credentials(String),
    /// The token is no longer good: sign in again.
    SessionExpired,
    /// The cloud said no (an order, a range); the text says why.
    Refused(String),
    /// The network, or an answer that is not what was expected.
    Network(anyhow::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credentials(why) => write!(f, "hitachi account refused the sign-in: {why}"),
            Self::SessionExpired => f.write_str("hitachi session expired"),
            Self::Refused(why) => write!(f, "hitachi refused: {why}"),
            Self::Network(e) => write!(f, "hitachi unreachable: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::Network(e)
    }
}

/// Who is signed in: the numbers the notification service wants.
#[derive(Clone, Copy, Debug)]
pub struct Account {
    pub user: i64,
    pub family: i64,
}

pub struct Client {
    email: String,
    password: String,
    token: Option<(String, Instant)>,
    last_request: Option<Instant>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never an identifier, a password or a token.
        f.debug_struct("Client")
            .field("has_session", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl Client {
    #[must_use]
    pub fn new(email: &str, password: &str) -> Self {
        Self {
            email: email.to_owned(),
            password: password.to_owned(),
            token: None,
            last_request: None,
        }
    }

    /// The access token, when it is good for a little while.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token
            .as_ref()
            .filter(|(_, until)| Instant::now() + RENEW_BEFORE < *until)
            .map(|(t, _)| t.as_str())
    }

    /// The token is dropped (it ended early): the next call signs in again.
    pub fn forget_session(&mut self) {
        self.token = None;
    }

    /// When the session should be renewed.
    #[must_use]
    pub fn renew_at(&self) -> Instant {
        self.token.as_ref().map_or_else(Instant::now, |(_, until)| {
            until.checked_sub(RENEW_BEFORE).unwrap_or_else(Instant::now)
        })
    }

    async fn call(
        &mut self,
        method: Method,
        path: &str,
        body: Option<&Json>,
        authenticated: bool,
    ) -> Result<(http::StatusCode, Json), ApiError> {
        if let Some(last) = self.last_request {
            let wait = MIN_GAP.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let mut request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        if authenticated {
            let Some((token, _)) = &self.token else {
                return Err(ApiError::SessionExpired);
            };
            request = request.header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| anyhow!("{e}"))?,
            );
        }
        let payload = body.map_or_else(Vec::new, |b| b.to_string().into_bytes());
        let request = request
            .body(Full::new(Body::from(payload)))
            .map_err(|e| anyhow!("{e}"))?;
        self.last_request = Some(Instant::now());
        let (status, _, bytes) =
            moli_net::web_with_headers(API_HOST, 443, true, request, REQUEST_LIMIT, MAX_ANSWER)
                .await?;
        if authenticated && status == http::StatusCode::UNAUTHORIZED {
            return Err(ApiError::SessionExpired);
        }
        // Some answers have no body (202): that is `null`, not an error.
        let answer = if bytes.is_empty() {
            Json::Null
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::Network(anyhow!("HTTP {status}, no JSON")))?
        };
        Ok((status, answer))
    }

    /// One sign-in. Never repeated here.
    pub async fn sign_in(&mut self) -> Result<(), ApiError> {
        let body = json!({"email": self.email, "password": self.password});
        let (status, answer) = self
            .call(Method::POST, "/iam/auth/sign-in", Some(&body), false)
            .await?;
        if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::BAD_REQUEST {
            let why = answer["errorState"]
                .as_str()
                .unwrap_or("refused")
                .to_owned();
            return Err(ApiError::Credentials(why));
        }
        let Some(token) = answer["token"].as_str().filter(|t| !t.is_empty()) else {
            return Err(ApiError::Network(anyhow!(
                "sign-in: HTTP {status}, no token"
            )));
        };
        let lifetime = answer["access_token_expires_in"]
            .as_u64()
            .map_or(DEFAULT_LIFETIME, Duration::from_millis);
        self.token = Some((token.to_owned(), Instant::now() + lifetime));
        Ok(())
    }

    /// The numbers of the signed-in user and of their home.
    pub async fn account(&mut self, family: Option<i64>) -> Result<Account, ApiError> {
        let (status, me) = self
            .call(Method::GET, "/iam/user/v2/who-am-i", None, true)
            .await?;
        let user = me["id"].as_i64();
        let family = family.or_else(|| me["familyId"].as_i64());
        match (user, family) {
            (Some(user), Some(family)) => Ok(Account { user, family }),
            _ => Err(ApiError::Network(anyhow!(
                "who-am-i: HTTP {status}, no user or home"
            ))),
        }
    }

    /// What the account's models take (modes, ranges, speeds).
    pub async fn capabilities(&mut self, family: i64) -> Result<Capabilities, ApiError> {
        let (_, ids) = self
            .call(
                Method::GET,
                &format!("/rac/ownership/groups/cloudIds/{family}"),
                None,
                true,
            )
            .await?;
        if !ids.is_array() {
            return Err(ApiError::Network(anyhow!("cloudIds: not a list")));
        }
        let (_, config) = self
            .call(
                Method::POST,
                "/rac/model-wise/rac-configuration/",
                Some(&ids),
                true,
            )
            .await?;
        Ok(Capabilities::from_json(&config))
    }

    /// The token, for an order that goes on its own (see [`send_order`]).
    #[must_use]
    pub fn bearer(&self) -> Option<String> {
        self.token().map(str::to_owned)
    }
}

/// One order: the unit's whole state. `Ok` once the cloud has taken it (the
/// unit follows a few seconds later). Independent of the client so that it can
/// run while the driver goes on reading and taking orders.
pub async fn send_order(token: String, unit: Unit, family: i64) -> Result<(), ApiError> {
    let path = format!(
        "/rac/basic-idu-control/general-control-command-status/{}?familyId={family}",
        unit.id
    );
    let payload = order_body(&unit).to_string().into_bytes();
    let request = http::Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| anyhow!("{e}"))?,
        )
        .body(Full::new(Body::from(payload)))
        .map_err(|e| anyhow!("{e}"))?;
    let (status, _, bytes) =
        moli_net::web_with_headers(API_HOST, 443, true, request, ORDER_LIMIT, MAX_ANSWER).await?;
    if status == http::StatusCode::UNAUTHORIZED {
        return Err(ApiError::SessionExpired);
    }
    let answer: Json = serde_json::from_slice(&bytes).unwrap_or(Json::Null);
    if !status.is_success() {
        let why = answer["message"]
            .as_str()
            .map_or_else(|| format!("HTTP {status}"), str::to_owned);
        return Err(ApiError::Refused(why));
    }
    match answer["status"].as_str() {
        Some("DONE") | None => Ok(()),
        Some(other) => Err(ApiError::Refused(format!("status {other}"))),
    }
}

/// The notification service, STOMP over WebSocket.
pub struct Feed {
    socket: WebSocket,
    token: String,
    account: Account,
    last_sent: Instant,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed").finish_non_exhaustive()
    }
}

/// What the feed delivered.
#[derive(Debug)]
pub enum Event {
    /// Nothing within the limit.
    Quiet,
    /// A heart-beat or a frame of no interest: the service is alive.
    Alive,
    Units(Json),
}

impl Feed {
    /// Connects, subscribes to the home's channel and asks for a first
    /// picture.
    pub async fn open(token: &str, account: Account) -> anyhow::Result<Self> {
        let socket = WebSocket::connect_public(
            NOTIFICATION_HOST,
            443,
            SOCKET_PATH,
            &[(
                "Origin",
                "https://notification-global-prod.aircloudhome.com",
            )],
        )
        .await?;
        let mut feed = Self {
            socket,
            token: token.to_owned(),
            account,
            last_sent: Instant::now(),
        };
        feed.socket.send_text(&stomp::connect(token)).await?;
        loop {
            let text = feed.socket.recv_text(Duration::from_secs(10)).await?;
            match stomp::parse(&text) {
                Frame::Frame { command, .. } if command == "CONNECTED" => break,
                Frame::Frame { command, body, .. } if command == "ERROR" => {
                    anyhow::bail!("notification service refused the connection: {body}");
                }
                _ => {}
            }
        }
        let destination = stomp::notifications(account.user, account.family);
        feed.socket
            .send_text(&stomp::subscribe(&destination, token))
            .await?;
        feed.refresh().await?;
        Ok(feed)
    }

    /// Asks for a fresh picture of all units.
    pub async fn refresh(&mut self) -> anyhow::Result<()> {
        let frame = stomp::send(
            &stomp::requests(self.account.user, self.account.family),
            &json!({"racId": 1, "requestType": "REFRESH_ALL"}),
            &self.token,
        );
        self.socket.send_text(&frame).await?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// The next thing the service says, within `limit`; a line feed goes
    /// back when the service has not heard from us for a while.
    pub async fn next(&mut self, limit: Duration) -> anyhow::Result<Event> {
        if self.last_sent.elapsed() > Duration::from_secs(10) {
            self.socket.send_text("\n").await?;
            self.last_sent = Instant::now();
        }
        let text = match self.socket.recv_text(limit).await {
            Ok(text) => text,
            Err(e) if e.to_string().contains("in time") => return Ok(Event::Quiet),
            Err(e) => return Err(e),
        };
        match stomp::parse(&text) {
            Frame::HeartBeat => Ok(Event::Alive),
            Frame::Frame { command, body, .. } => match command.as_str() {
                "MESSAGE" => serde_json::from_str(&body)
                    .map(Event::Units)
                    .map_err(|_| anyhow!("notification service: a message that is not JSON")),
                "ERROR" => Err(anyhow!("notification service error: {body}")),
                _ => Ok(Event::Alive),
            },
        }
    }
}
