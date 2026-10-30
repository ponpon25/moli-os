//! The Netatmo Connect API (`api.netatmo.com`), the part a doorbell needs:
//! the OAuth2 token exchange, the list of doorbells, their events and their
//! reachability.
//!
//! Rules: the refresh token is single-use (every exchange returns a new
//! one, the old one dies), so the new one is handed back to the caller to be
//! stored *before* anything else is done with it; a refused exchange is
//! reported, never retried here (the caller parks).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use http::{HeaderName, HeaderValue, Method};
use http_body_util::Full;
use moli_net::Body;
use serde_json::Value as Json;

const HOST: &str = "api.netatmo.com";
const REQUEST_LIMIT: Duration = Duration::from_secs(15);
const MAX_ANSWER: usize = 1024 * 1024;
/// The token is renewed this long before it ends.
const RENEW_BEFORE: Duration = Duration::from_secs(5 * 60);
const DEFAULT_LIFETIME: Duration = Duration::from_secs(3 * 3600);

/// Why a call did not go through.
#[derive(Debug)]
pub enum ApiError {
    /// The app or the refresh token was refused: a person acts.
    Credentials(String),
    /// The access token is no longer good: exchange again.
    SessionExpired,
    /// Netatmo said no (rate limit, bad request); the text says why.
    Refused(String),
    /// The network, or an answer that is not what was expected.
    Network(anyhow::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credentials(why) => write!(f, "netatmo refused the credentials: {why}"),
            Self::SessionExpired => f.write_str("netatmo session expired"),
            Self::Refused(why) => write!(f, "netatmo refused: {why}"),
            Self::Network(e) => write!(f, "netatmo unreachable: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::Network(e)
    }
}

/// A doorbell of a home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bell {
    /// Its module identifier (a MAC address).
    pub id: String,
    pub home: String,
    pub name: String,
}

pub struct Client {
    app_id: String,
    app_secret: String,
    refresh_token: String,
    access: Option<(String, Instant)>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never an identifier, a secret or a token.
        f.debug_struct("Client")
            .field("has_session", &self.access.is_some())
            .finish_non_exhaustive()
    }
}

/// Percent-encodes a form value.
fn form_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

impl Client {
    #[must_use]
    pub fn new(client_id: &str, client_secret: &str, refresh_token: &str) -> Self {
        Self {
            app_id: client_id.to_owned(),
            app_secret: client_secret.to_owned(),
            refresh_token: refresh_token.to_owned(),
            access: None,
        }
    }

    /// True when the access token is good for a little while.
    #[must_use]
    pub fn has_session(&self) -> bool {
        self.access
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() + RENEW_BEFORE < *until)
    }

    /// The access token is dropped (it ended early): the next call exchanges again.
    pub fn forget_session(&mut self) {
        self.access = None;
    }

    /// One token exchange. Returns the new refresh token, which the caller
    /// stores at once: the one used is dead. Never repeated here.
    pub async fn exchange(&mut self) -> Result<String, ApiError> {
        let body = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
            form_escape(&self.refresh_token),
            form_escape(&self.app_id),
            form_escape(&self.app_secret),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("/oauth2/token")
            .header("accept", "application/json")
            .header(
                "content-type",
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .body(Full::new(Body::from(body.into_bytes())))
            .map_err(|e| anyhow!("{e}"))?;
        let (status, bytes) =
            moli_net::web(HOST, 443, true, request, REQUEST_LIMIT, MAX_ANSWER).await?;
        let answer: Json = serde_json::from_slice(&bytes).unwrap_or(Json::Null);
        if status == http::StatusCode::BAD_REQUEST || status == http::StatusCode::UNAUTHORIZED {
            let why = answer["error"].as_str().unwrap_or("refused").to_owned();
            return Err(ApiError::Credentials(why));
        }
        let (Some(access), Some(refresh)) = (
            answer["access_token"].as_str().filter(|t| !t.is_empty()),
            answer["refresh_token"].as_str().filter(|t| !t.is_empty()),
        ) else {
            return Err(ApiError::Network(anyhow!(
                "token exchange: HTTP {status}, no token"
            )));
        };
        let lifetime = answer["expires_in"]
            .as_u64()
            .map_or(DEFAULT_LIFETIME, Duration::from_secs);
        self.access = Some((access.to_owned(), Instant::now() + lifetime));
        refresh.clone_into(&mut self.refresh_token);
        Ok(refresh.to_owned())
    }

    async fn get(&self, path: &str) -> Result<Json, ApiError> {
        let Some((token, _)) = &self.access else {
            return Err(ApiError::SessionExpired);
        };
        let request = http::Request::builder()
            .method(Method::GET)
            .uri(path)
            .header("accept", "application/json")
            .header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| anyhow!("{e}"))?,
            )
            .body(Full::new(Body::new()))
            .map_err(|e| anyhow!("{e}"))?;
        let (status, bytes) =
            moli_net::web(HOST, 443, true, request, REQUEST_LIMIT, MAX_ANSWER).await?;
        let answer: Json = serde_json::from_slice(&bytes).unwrap_or(Json::Null);
        if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::FORBIDDEN {
            // Netatmo codes 2 (invalid token) and 3 (expired token) land here.
            return Err(ApiError::SessionExpired);
        }
        if !status.is_success() {
            let why = answer["error"]["message"]
                .as_str()
                .map_or_else(|| format!("HTTP {status}"), str::to_owned);
            return Err(ApiError::Refused(why));
        }
        Ok(answer)
    }

    /// Tells Netatmo where to push its events (`url`, HTTPS). Netatmo drops
    /// the address after repeated failures, so it is given again now and then.
    pub async fn add_webhook(&self, url: &str) -> Result<(), ApiError> {
        let Some((token, _)) = &self.access else {
            return Err(ApiError::SessionExpired);
        };
        let body = format!("url={}", form_escape(url));
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("/api/addwebhook")
            .header("accept", "application/json")
            .header(
                "content-type",
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| anyhow!("{e}"))?,
            )
            .body(Full::new(Body::from(body.into_bytes())))
            .map_err(|e| anyhow!("{e}"))?;
        let (status, bytes) =
            moli_net::web(HOST, 443, true, request, REQUEST_LIMIT, MAX_ANSWER).await?;
        if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::FORBIDDEN {
            return Err(ApiError::SessionExpired);
        }
        if !status.is_success() {
            let answer: Json = serde_json::from_slice(&bytes).unwrap_or(Json::Null);
            let why = answer["error"]["message"]
                .as_str()
                .map_or_else(|| format!("HTTP {status}"), str::to_owned);
            return Err(ApiError::Refused(why));
        }
        Ok(())
    }

    /// The doorbells (module type `NDB`) of every home of the account.
    pub async fn doorbells(&self) -> Result<Vec<Bell>, ApiError> {
        let answer = self.get("/api/homesdata").await?;
        Ok(bells_of(&answer))
    }

    /// The latest events of a home, newest first as Netatmo gives them.
    pub async fn events(&self, home: &str, size: u32) -> Result<Vec<Json>, ApiError> {
        let path = format!("/api/getevents?home_id={}&size={size}", form_escape(home));
        let answer = self.get(&path).await?;
        Ok(answer["body"]["home"]["events"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Which modules of a home answer, by module identifier.
    pub async fn reachable(&self, home: &str) -> Result<Vec<(String, bool)>, ApiError> {
        let path = format!("/api/homestatus?home_id={}", form_escape(home));
        let answer = self.get(&path).await?;
        Ok(reachability_of(&answer))
    }
}

/// The doorbells in a `homesdata` answer.
#[must_use]
pub fn bells_of(answer: &Json) -> Vec<Bell> {
    let mut bells = Vec::new();
    for home in answer["body"]["homes"].as_array().into_iter().flatten() {
        let Some(home_id) = home["id"].as_str() else {
            continue;
        };
        for module in home["modules"].as_array().into_iter().flatten() {
            if module["type"].as_str() != Some("NDB") {
                continue;
            }
            let Some(id) = module["id"].as_str() else {
                continue;
            };
            bells.push(Bell {
                id: id.to_owned(),
                home: home_id.to_owned(),
                name: module["name"].as_str().unwrap_or("Sonnette").to_owned(),
            });
        }
    }
    bells
}

/// The modules' reachability in a `homestatus` answer.
#[must_use]
pub fn reachability_of(answer: &Json) -> Vec<(String, bool)> {
    answer["body"]["home"]["modules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            Some((id.to_owned(), m["reachable"].as_bool().unwrap_or(true)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_values_are_escaped() {
        assert_eq!(form_escape("5a|b c+d"), "5a%7Cb%20c%2Bd");
        assert_eq!(form_escape("abc-_.~9"), "abc-_.~9");
    }

    #[test]
    fn doorbells_are_picked_out() {
        let answer: Json = serde_json::from_str(include_str!(
            "../../../integrations/netatmo/fixtures/homesdata.json"
        ))
        .unwrap();
        let bells = bells_of(&answer);
        assert_eq!(bells.len(), 1);
        assert_eq!(bells[0].id, "aa:bb:cc:dd:ee:ff");
        assert_eq!(bells[0].home, "5e0000000000000000000001");
    }

    #[test]
    fn reachability_defaults_to_yes() {
        let answer: Json = serde_json::from_str(
            r#"{"body":{"home":{"modules":[{"id":"a","reachable":false},{"id":"b"}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            reachability_of(&answer),
            vec![("a".to_owned(), false), ("b".to_owned(), true)]
        );
    }
}
