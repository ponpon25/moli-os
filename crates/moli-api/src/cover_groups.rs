//! The groups of shutters, as the dashboard makes them (crate `moli-covers`):
//!
//! - `GET /api/cover-groups`: the groups, each with its device.
//! - `POST /api/cover-groups` `{ name, members, switches? }`: a new group.
//! - `PUT /api/cover-groups/{id}` `{ name, members, switches? }`: renamed,
//!   other shutters; `switches` left out keeps those it has.
//!
//! A switch is `{ device, open?, close?, stop? }`: the presses (values of its
//! `action` point) that open, close or stop the group.
//! - `DELETE /api/cover-groups/{id}`
//!
//! The writes are the household's, like the rest of how the dashboard is
//! arranged: a group gives nobody a power they did not have, since the guard
//! judges its orders on the rooms of its members. Each one goes to the journal.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use moli_core::{CoverGroupChange, Origin};
use moli_covers::{Group, GroupError, Groups, Switch};
use moli_runtime::{Hub, LabelPatch};
use serde::Deserialize;
use serde_json::json;

use crate::caller::Caller;
use crate::session::Sessions;

#[derive(Clone)]
pub(crate) struct CoverGroupsGate(pub(crate) Option<Groups>);

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn absent() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        &moli_i18n::tr!("serveur.groupes_stores.absent"),
    )
}

/// `Some(response)` when the caller may not change the groups.
fn refused(caller: &Caller) -> Option<Response> {
    let guest = caller
        .person
        .as_ref()
        .is_some_and(crate::people::PersonRef::is_guest);
    (!caller.is_household() || guest).then(|| {
        error(
            StatusCode::FORBIDDEN,
            &moli_i18n::tr!("serveur.groupes_stores.foyer"),
        )
    })
}

fn failed(e: &GroupError) -> Response {
    match e {
        GroupError::Invalid(why) => error(StatusCode::UNPROCESSABLE_ENTITY, why),
        GroupError::UnknownGroup => error(StatusCode::NOT_FOUND, &e.to_string()),
        GroupError::Io(_) => {
            tracing::error!(error = %e, "cover groups: not kept (cover-groups.json)");
            error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

fn view(group: &Group) -> serde_json::Value {
    json!({
        "id": group.id,
        "device": group.device_id().as_str(),
        "name": group.name,
        "members": group.members,
        "switches": group.switches,
    })
}

/// Journals the change, under who made it.
fn journal(
    hub: &Hub,
    humans: &Sessions,
    caller: &Caller,
    headers: &HeaderMap,
    group: &Group,
    change: CoverGroupChange,
) {
    hub.record_cover_group(
        crate::rest::origin(hub, headers, humans, caller),
        Some(caller.describe()),
        &group.id,
        &group.name,
        change,
        group.members.clone(),
    );
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Body {
    name: String,
    members: Vec<String>,
    #[serde(default)]
    switches: Option<Vec<Switch>>,
}

pub(crate) async fn list(Extension(CoverGroupsGate(gate)): Extension<CoverGroupsGate>) -> Response {
    let groups = gate.map(|g| g.list()).unwrap_or_default();
    Json(json!({ "groups": groups.iter().map(view).collect::<Vec<_>>() })).into_response()
}

pub(crate) async fn create(
    State(hub): State<Hub>,
    Extension(CoverGroupsGate(gate)): Extension<CoverGroupsGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<Body>,
) -> Response {
    let Some(groups) = gate else { return absent() };
    if let Some(refusal) = refused(&caller) {
        return refusal;
    }
    match groups.create_with(
        &body.name,
        &body.members,
        body.switches.as_deref().unwrap_or_default(),
    ) {
        Ok(group) => {
            journal(
                &hub,
                &humans,
                &caller,
                &headers,
                &group,
                CoverGroupChange::Created,
            );
            (StatusCode::CREATED, Json(view(&group))).into_response()
        }
        Err(e) => failed(&e),
    }
}

pub(crate) async fn update(
    State(hub): State<Hub>,
    Extension(CoverGroupsGate(gate)): Extension<CoverGroupsGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Body>,
) -> Response {
    let Some(groups) = gate else { return absent() };
    if let Some(refusal) = refused(&caller) {
        return refusal;
    }
    let group = match groups.update_with(&id, &body.name, &body.members, body.switches.as_deref()) {
        Ok(group) => group,
        Err(e) => return failed(&e),
    };
    journal(
        &hub,
        &humans,
        &caller,
        &headers,
        &group,
        CoverGroupChange::Updated,
    );
    // A name given in Équipements would hide the new one everywhere.
    let device = group.device_id();
    let label = hub.device(&device).and_then(|v| v.label.name.clone());
    if label.is_some_and(|name| name != group.name) {
        let patch = LabelPatch {
            name: Some(group.name.clone()),
            ..LabelPatch::default()
        };
        if let Err(e) = hub
            .set_label(&device, patch, Origin::System, Some(caller.describe()))
            .await
        {
            tracing::warn!(error = %e, "cover groups: the label keeps the former name");
        }
    }
    Json(view(&group)).into_response()
}

pub(crate) async fn remove(
    State(hub): State<Hub>,
    Extension(CoverGroupsGate(gate)): Extension<CoverGroupsGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(groups) = gate else { return absent() };
    if let Some(refusal) = refused(&caller) {
        return refusal;
    }
    match groups.remove(&id) {
        Ok(group) => {
            journal(
                &hub,
                &humans,
                &caller,
                &headers,
                &group,
                CoverGroupChange::Deleted,
            );
            Json(json!({ "removed": group.id })).into_response()
        }
        Err(e) => failed(&e),
    }
}
