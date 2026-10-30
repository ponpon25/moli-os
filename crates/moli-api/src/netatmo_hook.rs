//! `POST /api/netatmo/{token}`: Netatmo's push, when something happens at
//! the door. It says nothing the house believes: it only wakes the `netatmo`
//! driver, which reads the events from Netatmo's own API, signed in. A
//! forged push can therefore ring nothing, and the token in the address
//! keeps strangers from even waking it.

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use moli_core::InstanceId;
use moli_runtime::Hub;

use crate::alexa_home::token_matches;

/// The driver's handle, when a `netatmo` driver runs.
#[derive(Clone)]
pub(crate) struct NetatmoGate(pub(crate) Option<moli_netatmo::Hook>);

pub(crate) async fn push(
    State(hub): State<Hub>,
    Extension(NetatmoGate(hook)): Extension<NetatmoGate>,
    Path(token): Path<String>,
) -> Response {
    let Some(hook) = hook else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let expected = hub.secret(
        &InstanceId::from(hook.instance()),
        moli_netatmo::WEBHOOK_TOKEN,
    );
    if !token_matches(expected.as_deref(), Some(&token)) {
        tracing::warn!("netatmo: push with a wrong token");
        return StatusCode::NOT_FOUND.into_response();
    }
    hook.wake();
    StatusCode::OK.into_response()
}
