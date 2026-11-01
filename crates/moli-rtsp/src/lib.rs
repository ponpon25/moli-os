//! A surface that serves chosen cameras again over RTSP, so a video
//! recorder (UniFi Protect, through an ONVIF layer, or any NVR) or a player
//! (VLC, ffmpeg) can show and keep what only Moli can read: a HomeKit
//! doorbell, for instance.
//!
//! - **Chosen one by one**: only the cameras listed in the config, each
//!   under its own path (`rtsp://<host>:8554/<path>`).
//! - **A password, always**: Digest (the password never travels), user
//!   `moli` by default, the password in Moli's secret store
//!   (`moli-os secrets set rtsp password`, then a restart), read at each
//!   request. No password: nobody is let in.
//! - **The guard**: a camera in a protected room is never served (as from
//!   HomeKit), and a stream is cut within five seconds if its room becomes
//!   protected.
//! - **Relayed, never transcoded**: the camera's H.264 goes out as it came,
//!   in new RTP packets, interleaved on the RTSP connection (TCP only). No
//!   sound yet.
//! - **Continuous**: a recorder watches for days; streams are renewed
//!   before a source lets them go, and opened again when they drop
//!   ([`feed`]).
//! - Bounded: eight clients at most, requests of 8 KiB, three wrong
//!   passwords per connection.

mod connection;
mod digest;
mod feed;
mod request;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use moli_core::{DeviceId, InstanceId};
use moli_runtime::Hub;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use connection::Shared;

/// Where the password lives: `moli-os secrets set rtsp password`.
pub const NAMESPACE: &str = "rtsp";
pub const SECRET_PASSWORD: &str = "password";
/// Clients served at once.
const MAX_CLIENTS: usize = 8;

/// `[rtsp]` in `moli.toml`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// The user name clients give.
    #[serde(default = "default_user")]
    pub user: String,
    /// The cameras served (`[[rtsp.camera]]`).
    #[serde(default, rename = "camera")]
    pub cameras: Vec<Camera>,
}

/// One camera served.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Its path in the URL: letters, digits, `-` and `_`.
    pub path: String,
    /// The Moli device (one that shows live video).
    pub device: DeviceId,
}

fn default_port() -> u16 {
    8554
}

fn default_user() -> String {
    "moli".to_owned()
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("bad camera path {0:?}: letters, digits, '-' and '_' only, 32 at most")]
    BadPath(String),
    #[error("camera path {0:?} given twice")]
    TwicePath(String),
    #[error("bad user name")]
    BadUser,
    #[error("cannot listen on port {0}: {1}")]
    Listen(u16, std::io::Error),
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 32
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Starts the surface; the task ends when `cancel` is.
pub async fn start(
    hub: Hub,
    config: Config,
    cancel: CancellationToken,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), StartError> {
    if hub
        .secret(&InstanceId::from(NAMESPACE), SECRET_PASSWORD)
        .is_none_or(|p| p.is_empty())
    {
        tracing::warn!(
            "rtsp: no password yet (moli-os secrets set rtsp password, then restart): nobody is let in"
        );
    }
    if config.user.is_empty()
        || config.user.len() > 64
        || config.user.contains(['"', ':', '\\'])
        || config.user.chars().any(char::is_control)
    {
        return Err(StartError::BadUser);
    }
    let mut cameras = HashMap::new();
    for camera in &config.cameras {
        if !valid_path(&camera.path) {
            return Err(StartError::BadPath(camera.path.clone()));
        }
        if cameras
            .insert(camera.path.clone(), camera.device.clone())
            .is_some()
        {
            return Err(StartError::TwicePath(camera.path.clone()));
        }
    }
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, config.port))
        .await
        .map_err(|e| StartError::Listen(config.port, e))?;
    let address = listener
        .local_addr()
        .map_err(|e| StartError::Listen(config.port, e))?;
    for (path, device) in &cameras {
        if !hub.has_live(device) {
            tracing::info!(%path, device = %device, "rtsp: no live video for this device yet");
        }
    }
    tracing::info!(
        port = address.port(),
        cameras = cameras.len(),
        "rtsp: serving"
    );
    let shared = Arc::new(Shared {
        hub,
        cameras,
        user: config.user,
        nonces: std::sync::Mutex::default(),
    });
    let slots = Arc::new(Semaphore::new(MAX_CLIENTS));
    let task = tokio::spawn(async move {
        loop {
            let (stream, peer) = tokio::select! {
                () = cancel.cancelled() => return,
                accepted = listener.accept() => match accepted {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(error = %e, "rtsp: accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        continue;
                    }
                },
            };
            let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                tracing::warn!(%peer, "rtsp: too many clients, refused");
                continue;
            };
            tracing::info!(%peer, "rtsp: client");
            let (shared, cancel) = (Arc::clone(&shared), cancel.clone());
            tokio::spawn(async move {
                connection::serve(stream, peer, shared, cancel).await;
                drop(slot);
            });
        }
    });
    Ok((address, task))
}
