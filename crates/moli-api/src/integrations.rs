//! Keys a person gives Moli for an integration (the Tuya cloud project's
//! Access ID and Secret): typed in the dashboard by a human (code), filed in
//! Moli's encrypted store under the driver's name, never sent back. The
//! driver, waiting for them, restarts with them.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use moli_core::InstanceId;
use moli_runtime::Hub;
use serde::Deserialize;
use serde_json::json;

use crate::caller::Caller;
use crate::session::Sessions;

const ID: &str = "cloud_access_id";
const SECRET: &str = "cloud_access_secret";

fn tuya_instances(hub: &Hub) -> Vec<InstanceId> {
    hub.snapshot()
        .drivers
        .iter()
        .filter(|d| d.kind == "tuya")
        .map(|d| d.instance.clone())
        .collect()
}

/// Whether each Tuya instance has cloud keys filed (never the keys).
pub(crate) async fn tuya_cloud(State(hub): State<Hub>) -> Response {
    let instances: Vec<_> = tuya_instances(&hub)
        .into_iter()
        .map(|i| {
            let configured = hub.secret(&i, ID).is_some() && hub.secret(&i, SECRET).is_some();
            json!({ "instance": i, "configured": configured })
        })
        .collect();
    Json(json!({ "instances": instances })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct Keys {
    #[serde(default)]
    instance: Option<String>,
    access_id: String,
    access_secret: String,
}

fn plausible(value: &str, min: usize) -> bool {
    (min..=64).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric())
}

pub(crate) async fn set_tuya_cloud(
    State(hub): State<Hub>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Json(keys): Json<Keys>,
) -> Response {
    let refuse = |status: StatusCode, message: &str| {
        (status, Json(json!({ "error": message }))).into_response()
    };
    if !humans.is_human(&headers, &caller.key) {
        return refuse(
            StatusCode::FORBIDDEN,
            &moli_i18n::tr!("serveur.cle.tuya_humain"),
        );
    }
    let instances = tuya_instances(&hub);
    let instance = match keys.instance.as_deref() {
        Some(name) => InstanceId::from(name),
        None => match instances.first() {
            Some(i) => i.clone(),
            None => {
                return refuse(
                    StatusCode::NOT_FOUND,
                    &moli_i18n::tr!("serveur.pilote.absent", pilote = "tuya"),
                );
            }
        },
    };
    if !instances.contains(&instance) {
        return refuse(
            StatusCode::NOT_FOUND,
            &moli_i18n::tr!("serveur.cle.tuya_pilote_inconnu"),
        );
    }
    let (id, secret) = (keys.access_id.trim(), keys.access_secret.trim());
    if !plausible(id, 10) || !plausible(secret, 24) {
        return refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            &moli_i18n::tr!("serveur.cle.tuya_invalide"),
        );
    }
    for (name, value) in [(ID, id), (SECRET, secret)] {
        if let Err(e) = hub.store_secret(&instance, name, value).await {
            return refuse(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }
    tracing::info!(%instance, "tuya cloud keys filed from the dashboard");
    Json(json!({ "stored": true, "instance": instance })).into_response()
}

// ---- Apple Music --------------------------------------------------------------

const APPLE_KEY: &str = "apple_music_key";
const APPLE_TEAM: &str = "apple_team_id";
const APPLE_KEY_ID: &str = "apple_key_id";
const APPLE_STOREFRONT: &str = "apple_storefront";
const APPLE_USER: &str = "apple_music_user_token";

fn sonos_instances(hub: &Hub) -> Vec<InstanceId> {
    hub.snapshot()
        .drivers
        .iter()
        .filter(|d| d.kind == "sonos")
        .map(|d| d.instance.clone())
        .collect()
}

/// The Sonos instance that has Apple Music set up (its key and its
/// identifiers, which the driver files itself).
fn apple_instance(hub: &Hub) -> Option<InstanceId> {
    sonos_instances(hub).into_iter().find(|i| {
        [APPLE_KEY, APPLE_TEAM, APPLE_KEY_ID]
            .iter()
            .all(|name| hub.secret(i, name).is_some())
    })
}

/// Whether Apple Music is set up, and whether a person's account is
/// connected (never the keys).
pub(crate) async fn apple_music(State(hub): State<Hub>) -> Response {
    let instance = apple_instance(&hub);
    let connected = instance
        .as_ref()
        .is_some_and(|i| hub.secret(i, APPLE_USER).is_some());
    Json(json!({
        "configured": instance.is_some(),
        "connected": connected,
        "instance": instance,
    }))
    .into_response()
}

/// What is answered to someone who is not a person at the dashboard.
fn not_a_person() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "error": moli_i18n::tr!("serveur.cle.apple_humain") })),
    )
        .into_response()
}

/// A short developer token, for the dashboard's sign-in to Apple (MusicKit
/// in the browser): signed here with the house's key, which never leaves.
pub(crate) async fn apple_developer_token(
    State(hub): State<Hub>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
) -> Response {
    if !humans.is_human(&headers, &caller.key) {
        return not_a_person();
    }
    let fail = |status: StatusCode, message: String| {
        (status, Json(json!({ "error": message }))).into_response()
    };
    let Some(instance) = apple_instance(&hub) else {
        return fail(
            StatusCode::NOT_FOUND,
            moli_i18n::tr!("serveur.cle.apple_absente"),
        );
    };
    let (Some(pem), Some(team), Some(key_id)) = (
        hub.secret(&instance, APPLE_KEY),
        hub.secret(&instance, APPLE_TEAM),
        hub.secret(&instance, APPLE_KEY_ID),
    ) else {
        return fail(
            StatusCode::NOT_FOUND,
            moli_i18n::tr!("serveur.cle.apple_absente"),
        );
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let token = moli_net::es256::Signer::from_pem(&pem)
        .and_then(|signer| signer.jwt(&key_id, &team, now, 3600));
    match token {
        Ok(token) => Json(json!({
            "token": token,
            "storefront": hub.secret(&instance, APPLE_STOREFRONT).unwrap_or_else(|| "fr".into()),
        }))
        .into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
pub(crate) struct AppleUser {
    user_token: String,
}

/// Files the person's Apple Music token (what the sign-in to Apple gave the
/// dashboard): in the encrypted store, never sent back.
pub(crate) async fn set_apple_user_token(
    State(hub): State<Hub>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<AppleUser>,
) -> Response {
    if !humans.is_human(&headers, &caller.key) {
        return not_a_person();
    }
    let token = body.user_token.trim();
    let plausible = (40..=4096).contains(&token.len())
        && token.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_' | b'.')
        });
    let refuse = |status: StatusCode, message: String| {
        (status, Json(json!({ "error": message }))).into_response()
    };
    let Some(instance) = apple_instance(&hub) else {
        return refuse(
            StatusCode::NOT_FOUND,
            moli_i18n::tr!("serveur.cle.apple_absente"),
        );
    };
    if !plausible {
        return refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            moli_i18n::tr!("serveur.cle.apple_invalide"),
        );
    }
    if let Err(e) = hub.store_secret(&instance, APPLE_USER, token).await {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    tracing::info!(%instance, "an Apple Music account was connected from the dashboard");
    Json(json!({ "stored": true })).into_response()
}

/// Forgets the connected account (the house's own key stays).
pub(crate) async fn forget_apple_user_token(
    State(hub): State<Hub>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
) -> Response {
    if !humans.is_human(&headers, &caller.key) {
        return not_a_person();
    }
    if let Some(instance) = apple_instance(&hub)
        && let Err(e) = hub.forget_secret(&instance, APPLE_USER).await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    Json(json!({ "stored": false })).into_response()
}
