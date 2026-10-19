//! The HomeKit bridge's settings (crate `moli-homekit`):
//!
//! - `GET /api/homekit`: the bridge's state and every device HomeKit has a
//!   place for, with whether it is shown. The setup code (and the QR
//!   payload) only goes to a human session.
//! - `PUT /api/homekit/policy` `{ policy: "all" | "none" }`
//! - `PUT /api/homekit/devices/{id}` `{ exposed: true | false | null }`
//!   (`null`: back to the policy)
//! - `POST /api/homekit/unpair`: forget every phone.
//! - `POST /api/homekit/reset`: a new bridge (new identity and code), to add
//!   again on any iPhone.
//!
//! The writes are a human's (code): they decide what leaves the house.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use moli_homekit::{HomeKit, Policy};
use serde::Deserialize;
use serde_json::json;

use crate::caller::Caller;
use crate::session::Sessions;

#[derive(Clone)]
pub(crate) struct HomeKitGate(pub(crate) Option<HomeKit>);

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn absent() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        &moli_i18n::tr!("serveur.homekit.absent"),
    )
}

pub(crate) async fn status(
    Extension(HomeKitGate(gate)): Extension<HomeKitGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
) -> Response {
    let Some(homekit) = gate else {
        return Json(json!({ "enabled": false })).into_response();
    };
    let reveal = humans.is_human(&headers, &caller.key);
    Json(json!({
        "enabled": true,
        "human": reveal,
        "status": homekit.status(reveal),
        "devices": homekit.devices(),
    }))
    .into_response()
}

/// `Some(response)` when the caller is not a human.
fn humans_only(humans: &Sessions, caller: &Caller, headers: &HeaderMap) -> Option<Response> {
    (!humans.is_human(headers, &caller.key)).then(|| {
        error(
            StatusCode::FORBIDDEN,
            &moli_i18n::tr!("serveur.homekit.humain"),
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyBody {
    policy: Policy,
}

pub(crate) async fn set_policy(
    Extension(HomeKitGate(gate)): Extension<HomeKitGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<PolicyBody>,
) -> Response {
    let Some(homekit) = gate else { return absent() };
    if let Some(refusal) = humans_only(&humans, &caller, &headers) {
        return refusal;
    }
    homekit.set_policy(body.policy);
    tracing::info!(policy = ?body.policy, "homekit: default policy changed");
    Json(json!({ "policy": body.policy })).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceBody {
    exposed: Option<bool>,
}

pub(crate) async fn set_device(
    Extension(HomeKitGate(gate)): Extension<HomeKitGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<DeviceBody>,
) -> Response {
    let Some(homekit) = gate else { return absent() };
    if let Some(refusal) = humans_only(&humans, &caller, &headers) {
        return refusal;
    }
    if !homekit.knows(&id) {
        return error(
            StatusCode::NOT_FOUND,
            &moli_i18n::tr!("serveur.homekit.appareil_inconnu"),
        );
    }
    homekit.set_device(&id, body.exposed);
    tracing::info!(device = %id, exposed = ?body.exposed, "homekit: a device's choice changed");
    Json(json!({ "id": id, "exposed": body.exposed })).into_response()
}

pub(crate) async fn unpair(
    Extension(HomeKitGate(gate)): Extension<HomeKitGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
) -> Response {
    let Some(homekit) = gate else { return absent() };
    if let Some(refusal) = humans_only(&humans, &caller, &headers) {
        return refusal;
    }
    homekit.forget_pairings();
    tracing::info!("homekit: every controller was unpaired from the dashboard");
    Json(json!({ "unpaired": true })).into_response()
}

pub(crate) async fn reset(
    Extension(HomeKitGate(gate)): Extension<HomeKitGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
) -> Response {
    let Some(homekit) = gate else { return absent() };
    if let Some(refusal) = humans_only(&humans, &caller, &headers) {
        return refusal;
    }
    match homekit.reset().await {
        Ok(()) => {
            tracing::info!("homekit: the bridge got a new identity and setup code");
            Json(json!({ "reset": true })).into_response()
        }
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}
