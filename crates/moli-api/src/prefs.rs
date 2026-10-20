//! How the family arranged the dashboard (its tiles, the rooms hidden or made
//! here, which devices show), kept in `prefs.json` next to the layout file so
//! that one device's choice holds on every device. Only the dashboard's own
//! keys are kept, each a JSON value of its own.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Map, Value, json};

use crate::caller::Caller;

/// Where the file is (`None`: the house has no data folder).
#[derive(Clone, Debug)]
pub(crate) struct PrefsFile(pub(crate) Option<PathBuf>);

/// The keys the dashboard shares across devices.
const KEYS: [&str; 5] = [
    "maison-dashboard",
    "maison-dashboard-off",
    "maison-dashboard-on",
    "maison-rooms-hidden",
    "maison-rooms-extra",
];

/// Largest value kept for one key (the tiles of a big house stay far under).
const MAX_VALUE: usize = 256 * 1024;

/// One writer at a time: a read-modify-write of the whole file.
static WRITING: Mutex<()> = Mutex::new(());

fn read(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn keep(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let _one = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut all = read(path);
    all.insert(key.to_owned(), value);
    let text = serde_json::to_string_pretty(&all).map_err(std::io::Error::other)?;
    // Written aside then renamed: never half a file.
    let aside = path.with_extension("json.new");
    std::fs::write(&aside, text)?;
    std::fs::rename(&aside, path)
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Everything the house kept (nothing yet: `{}`).
pub(crate) async fn get(Extension(PrefsFile(file)): Extension<PrefsFile>) -> Response {
    let all = file.as_deref().map(read).unwrap_or_default();
    Json(Value::Object(all)).into_response()
}

/// Keeps one key, for the household only.
pub(crate) async fn put(
    Extension(PrefsFile(file)): Extension<PrefsFile>,
    caller: Caller,
    UrlPath(key): UrlPath<String>,
    Json(value): Json<Value>,
) -> Response {
    if !caller.is_household()
        || caller
            .person
            .as_ref()
            .is_some_and(crate::people::PersonRef::is_guest)
    {
        return error(
            StatusCode::FORBIDDEN,
            "the household arranges the dashboard",
        );
    }
    if !KEYS.contains(&key.as_str()) {
        return error(StatusCode::NOT_FOUND, "unknown preference");
    }
    if value.to_string().len() > MAX_VALUE {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "preference too large");
    }
    let Some(path) = file else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no data folder to keep it in",
        );
    };
    match keep(&path, &key, value) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "dashboard preference not kept (prefs.json)");
            error(StatusCode::INTERNAL_SERVER_ERROR, "not kept")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_kept_with_the_others() {
        let dir = std::env::temp_dir().join(format!("moli-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prefs.json");
        assert!(read(&file).is_empty());
        keep(&file, "maison-rooms-hidden", json!(["Garage"])).unwrap();
        keep(&file, "maison-dashboard", json!({ "a": { "w": 6 } })).unwrap();
        keep(&file, "maison-rooms-hidden", json!([])).unwrap();
        let all = read(&file);
        assert_eq!(all["maison-rooms-hidden"], json!([]));
        assert_eq!(all["maison-dashboard"]["a"]["w"], 6);
        assert_eq!(all.len(), 2);
        // A damaged file reads as nothing, and is replaced by the next write.
        std::fs::write(&file, "{ not json").unwrap();
        assert!(read(&file).is_empty());
        keep(&file, "maison-dashboard-on", json!(["x"])).unwrap();
        assert_eq!(read(&file).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_dashboards_keys_are_shared() {
        assert!(KEYS.contains(&"maison-dashboard"));
        assert!(!KEYS.contains(&"maison-theme"));
        assert!(!KEYS.contains(&"../settings"));
    }
}
