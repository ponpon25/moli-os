//! The cover of what a room plays, through Moli: a browser away from home
//! (or on another network) cannot reach a speaker's address.

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context as _, bail};
use moli_net::upnp::{fetch_bytes, tag, unescape};
use moli_runtime::BoxFuture;
use moli_runtime::media::{CoverSource, Image};

use crate::{AV, PORT, soap};

/// Where a room's cover is read: the player that leads its group (the
/// followers play what it plays).
#[derive(Debug)]
pub(crate) struct Cover {
    pub(crate) host: Arc<Mutex<String>>,
}

impl CoverSource for Cover {
    fn cover(&self) -> BoxFuture<'_, anyhow::Result<Image>> {
        Box::pin(async move {
            let host = self
                .host
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            read(&host).await
        })
    }
}

async fn read(host: &str) -> anyhow::Result<Image> {
    let position = soap(host, AV, "GetPositionInfo", &[("InstanceID", "0")]).await?;
    let meta = unescape(tag(&position, "TrackMetaData").unwrap_or(""));
    let art = tag(&meta, "albumArtURI")
        .map(unescape)
        .context("nothing to show")?;
    let path = art_path(host, &art).context("a cover that is not the speaker's own")?;
    let bytes = fetch_bytes(host, PORT, &path).await?;
    let Some(content_type) = image_type(&bytes) else {
        bail!("the speaker did not send a picture");
    };
    Ok(Image {
        content_type: content_type.into(),
        bytes,
    })
}

/// The path of a cover the speaker itself serves (`/getaa…`), whether it
/// says it whole or by its path: nothing else is ever fetched.
fn art_path(host: &str, art: &str) -> Option<String> {
    let own = format!("http://{host}:{PORT}");
    let path = art.strip_prefix(own.as_str()).unwrap_or(art);
    path.starts_with("/getaa").then(|| path.to_owned())
}

fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_speakers_own_covers_are_fetched() {
        let host = "192.168.1.10";
        assert_eq!(
            art_path(host, "/getaa?s=1&u=x").as_deref(),
            Some("/getaa?s=1&u=x")
        );
        assert_eq!(
            art_path(host, "http://192.168.1.10:1400/getaa?v=0&vli=1").as_deref(),
            Some("/getaa?v=0&vli=1")
        );
        assert_eq!(art_path(host, "http://192.168.1.99:1400/getaa?x"), None);
        assert_eq!(art_path(host, "https://example.com/cover.jpg"), None);
        assert_eq!(art_path(host, "/xml/device_description.xml"), None);
    }

    #[test]
    fn only_pictures_are_passed_on() {
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"\x89PNG\r\n"), Some("image/png"));
        assert_eq!(image_type(b"<html>"), None);
        assert_eq!(image_type(b""), None);
    }
}
