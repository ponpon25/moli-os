//! Sonos players, through their local UPnP API (HTTP 1400): play/pause,
//! volume, mute, what is playing. Polled every two seconds. With
//! `[driver.options.announce]`, spoken announcements too (`announce.rs`),
//! and `quiet`: while on, no announcement is said (whoever asks), until it
//! is switched off or [`QUIET_FOR`] has passed (a silence forgotten on).
//! With `discover = true`, every player of the home at once, grouped by
//! room (`household.rs`).

mod actions;
mod announce;
mod apple;
mod cover;
mod events;
mod household;
mod playback;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_net::upnp::{tag, unescape};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;

const PORT: u16 = 1400;
const POLL: Duration = Duration::from_secs(2);
/// What a speaker is given to fill its queue.
const SLOW: Duration = Duration::from_secs(30);
/// A silence ends by itself after this long.
pub const QUIET_FOR: Duration = Duration::from_secs(4 * 3600);
const AV: (&str, &str) = (
    "/MediaRenderer/AVTransport/Control",
    "urn:schemas-upnp-org:service:AVTransport:1",
);
const RENDERING: (&str, &str) = (
    "/MediaRenderer/RenderingControl/Control",
    "urn:schemas-upnp-org:service:RenderingControl:1",
);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// One player (the driver is that player), or, with `discover`, an
    /// address to start from when the search finds nothing.
    #[serde(default)]
    pub host: Option<String>,
    /// Every player of the home, found by search, one device per room.
    #[serde(default)]
    pub discover: bool,
    /// With `discover`: players call back when something changes (read at
    /// once, and the safety round slows down). On by default.
    #[serde(default = "yes")]
    pub events: bool,
    /// Where they call back (this machine, any free port to give them).
    #[serde(default = "events_port")]
    pub events_port: u16,
    /// With `discover`: the Apple Music catalogue, searched from Moli and
    /// played by the speakers (the private key is the secret `apple_music_key`).
    #[serde(default)]
    pub apple: Option<apple::Config>,
    /// Spoken announcements (absent: none).
    #[serde(default)]
    pub announce: Option<announce::Config>,
}

fn yes() -> bool {
    true
}

fn events_port() -> u16 {
    3400
}

/// Where the speaker's local API certificate fingerprint is kept.
const WS_CERT: &str = "ws_cert";

#[derive(Debug)]
pub struct Sonos {
    config: Config,
}

impl Sonos {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Sonos {
    fn kind(&self) -> &'static str {
        "sonos"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if self.config.discover {
                household::run(&self.config, ctx).await
            } else {
                run(&self.config, ctx).await
            }
        })
    }
}

async fn soap(
    host: &str,
    (path, service): (&str, &str),
    action: &str,
    args: &[(&str, &str)],
) -> anyhow::Result<String> {
    moli_net::upnp::soap(host, PORT, path, service, action, args).await
}

/// An action that takes a while (a long playlist put in the queue).
async fn soap_slow(
    host: &str,
    (path, service): (&str, &str),
    action: &str,
    args: &[(&str, &str)],
) -> anyhow::Result<String> {
    moli_net::upnp::soap_within(host, PORT, path, service, action, args, SLOW).await
}

fn point(
    key: &str,
    label: &str,
    kind: Kind,
    write: bool,
    unit: Option<Unit>,
    semantic: Semantic,
) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access { read: true, write },
        unit,
        semantic,
    }
}

#[allow(clippy::too_many_lines)] // one entry per point
fn device(ctx: &DriverCtx, id: &DeviceId, room: &str, model: &str, speaks: bool) -> Device {
    let mut device = Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: room.into(),
        manufacturer: Some("Sonos".into()),
        model: Some(model.into()),
        description: None,
        native_room: Some(room.into()),
        members: Vec::new(),
        points: vec![
            point(
                "playing",
                &moli_i18n::tr!("pilotes.sonos.lecture"),
                Kind::Binary,
                true,
                None,
                Semantic::OnOff,
            ),
            point(
                "volume",
                &moli_i18n::tr!("pilotes.sonos.volume"),
                Kind::Numeric {
                    min: Some(0.0),
                    max: Some(100.0),
                    step: Some(1.0),
                },
                true,
                Some(Unit::Percent),
                Semantic::infer("volume", None),
            ),
            point(
                "mute",
                &moli_i18n::tr!("pilotes.sonos.sourdine"),
                Kind::Binary,
                true,
                None,
                Semantic::infer("mute", None),
            ),
            point(
                "state",
                &moli_i18n::tr!("pilotes.sonos.etat"),
                Kind::Text,
                false,
                None,
                Semantic::Other,
            ),
            point(
                "title",
                &moli_i18n::tr!("pilotes.sonos.titre"),
                Kind::Text,
                false,
                None,
                Semantic::Other,
            ),
            point(
                "artist",
                &moli_i18n::tr!("pilotes.sonos.artiste"),
                Kind::Text,
                false,
                None,
                Semantic::Other,
            ),
            point(
                "source",
                &moli_i18n::tr!("pilotes.sonos.source"),
                Kind::Enum {
                    values: SOURCES.iter().map(|s| (*s).into()).collect(),
                },
                false,
                None,
                Semantic::Other,
            ),
        ],
    };
    if speaks {
        device.points.push(PointSpec {
            key: "quiet".into(),
            label: moli_i18n::tr!("pilotes.sonos.annonces_coupees").into(),
            kind: Kind::Binary,
            access: Access {
                read: true,
                write: true,
            },
            unit: None,
            semantic: Semantic::Other,
        });
        device.points.push(PointSpec {
            key: "quiet_until".into(),
            label: moli_i18n::tr!("pilotes.sonos.annonces_coupees_jusqua").into(),
            kind: Kind::Numeric {
                min: None,
                max: None,
                step: None,
            },
            access: Access {
                read: true,
                write: false,
            },
            unit: None,
            semantic: Semantic::Other,
        });
        device.points.push(PointSpec {
            key: "announce".into(),
            label: moli_i18n::tr!("pilotes.sonos.annonce").into(),
            kind: Kind::Text,
            // Words to say, not a state: nothing to read back.
            access: Access {
                read: false,
                write: true,
            },
            unit: None,
            semantic: Semantic::Other,
        });
    }
    device
}

async fn describe(host: &str) -> anyhow::Result<(String, String, String)> {
    let xml = moli_net::upnp::fetch(host, PORT, "/xml/device_description.xml").await?;
    let udn = tag(&xml, "UDN")
        .context("no UDN")?
        .trim_start_matches("uuid:")
        .to_owned();
    let room = tag(&xml, "roomName").unwrap_or("Sonos").to_owned();
    let model = tag(&xml, "modelName").unwrap_or("Sonos").to_owned();
    Ok((udn, room, model))
}

/// Where the sound comes from, as the remote needs it: `tv` (a soundbar's
/// TV input: the TV's volume is then the speaker's), `line_in`, `airplay`,
/// `bluetooth`, `radio`, `group` (it follows another player), `music`, or
/// `none`.
const SOURCES: [&str; 8] = [
    "tv",
    "line_in",
    "airplay",
    "bluetooth",
    "radio",
    "group",
    "music",
    "none",
];

/// The source of a track, by its address.
fn source(uri: &str) -> &'static str {
    let uri = uri.trim();
    if uri.is_empty() {
        "none"
    } else if uri.starts_with("x-sonos-htastream:") {
        "tv"
    } else if uri.starts_with("x-rincon-stream:") {
        "line_in"
    } else if uri.starts_with("x-rincon:") {
        "group"
    } else if uri.starts_with("x-sonos-vli:") && uri.contains(",airplay:") {
        "airplay"
    } else if uri.starts_with("x-sonos-vli:") && uri.contains(",bluetooth:") {
        "bluetooth"
    } else if uri.starts_with("x-sonosapi-stream:")
        || uri.starts_with("x-sonosapi-radio:")
        || uri.starts_with("x-sonosapi-hls:")
        || uri.starts_with("x-rincon-mp3radio:")
    {
        "radio"
    } else {
        "music"
    }
}

/// `H:MM:SS` as seconds (`NOT_IMPLEMENTED`, empty: none).
fn seconds(text: &str) -> Option<i64> {
    let mut parts = text.trim().split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60 + s.parse::<i64>().ok()?)
}

/// What a player says now.
struct Polled {
    values: Vec<(&'static str, Value)>,
    /// Where in the track, in seconds (`full` readings only).
    position: Option<i64>,
    /// The track's address: a change of it is a change of track.
    track: String,
}

/// Reads a player. `full` adds what a room shows beyond the remote's needs:
/// album, cover, length.
async fn poll(host: &str, full: bool) -> anyhow::Result<Polled> {
    let transport = soap(host, AV, "GetTransportInfo", &[("InstanceID", "0")]).await?;
    let state = tag(&transport, "CurrentTransportState")
        .unwrap_or("UNKNOWN")
        .to_owned();
    let volume = soap(
        host,
        RENDERING,
        "GetVolume",
        &[("InstanceID", "0"), ("Channel", "Master")],
    )
    .await?;
    let mute = soap(
        host,
        RENDERING,
        "GetMute",
        &[("InstanceID", "0"), ("Channel", "Master")],
    )
    .await?;
    let position = soap(host, AV, "GetPositionInfo", &[("InstanceID", "0")]).await?;
    let meta = unescape(tag(&position, "TrackMetaData").unwrap_or(""));
    let track = unescape(tag(&position, "TrackURI").unwrap_or(""));
    let from = source(&track);
    let text = |s: Option<&str>| {
        s.map(|s| unescape(s).trim().to_owned())
            .filter(|s| !s.is_empty() && s != "NOT_IMPLEMENTED")
    };
    let as_text = |t: Option<String>| t.map_or(Value::Null, |t| Value::Text(t.into()));
    let mut values = vec![
        (
            "playing",
            Value::Bool(state == "PLAYING" || state == "TRANSITIONING"),
        ),
        ("state", Value::Text(state.to_lowercase().into())),
        ("source", Value::Text(from.into())),
        ("mute", Value::Bool(tag(&mute, "CurrentMute") == Some("1"))),
        ("title", as_text(text(tag(&meta, "title")))),
        ("artist", as_text(text(tag(&meta, "creator")))),
    ];
    if let Some(v) = tag(&volume, "CurrentVolume").and_then(|v| v.parse::<i64>().ok()) {
        values.push(("volume", Value::Int(v)));
    }
    let mut at = None;
    if full {
        let settings = soap(host, AV, "GetTransportSettings", &[("InstanceID", "0")]).await?;
        let (shuffle, repeat) =
            playback::mode_parts(tag(&settings, "PlayMode").unwrap_or("NORMAL"));
        values.push(("shuffle", Value::Bool(shuffle)));
        values.push(("repeat", Value::Text(repeat.into())));
        // The queue counts only while it is what plays.
        let media = soap(host, AV, "GetMediaInfo", &[("InstanceID", "0")]).await?;
        let in_queue = tag(&media, "CurrentURI")
            .is_some_and(|uri| unescape(uri).starts_with("x-rincon-queue:"));
        let number = |text: Option<&str>| {
            text.and_then(|t| t.trim().parse::<i64>().ok())
                .filter(|n| *n > 0)
                .filter(|_| in_queue)
                .map_or(Value::Null, Value::Int)
        };
        values.push(("queue_length", number(tag(&media, "NrTracks"))));
        values.push(("queue_position", number(tag(&position, "Track"))));
        values.push(("album", as_text(text(tag(&meta, "album")))));
        values.push((
            "art",
            as_text(text(tag(&meta, "albumArtURI")).map(|art| cover(host, &art))),
        ));
        let length = tag(&position, "TrackDuration")
            .and_then(seconds)
            .filter(|s| *s > 0);
        values.push(("duration", length.map_or(Value::Null, Value::Int)));
        at = length.and(tag(&position, "RelTime").and_then(seconds));
    }
    Ok(Polled {
        values,
        position: at,
        track,
    })
}

/// A cover's address, whole (a player gives a path of its own).
fn cover(host: &str, art: &str) -> String {
    if art.starts_with('/') {
        format!("http://{host}:{PORT}{art}")
    } else {
        art.to_owned()
    }
}

async fn apply(host: &str, command: &CommandRequest) -> anyhow::Result<()> {
    match (&*command.key, &command.value) {
        ("playing", Value::Bool(true)) => {
            soap(host, AV, "Play", &[("InstanceID", "0"), ("Speed", "1")]).await?
        }
        ("playing", Value::Bool(false)) => soap(host, AV, "Pause", &[("InstanceID", "0")]).await?,
        ("mute", Value::Bool(m)) => {
            soap(
                host,
                RENDERING,
                "SetMute",
                &[
                    ("InstanceID", "0"),
                    ("Channel", "Master"),
                    ("DesiredMute", if *m { "1" } else { "0" }),
                ],
            )
            .await?
        }
        ("volume", v) => {
            #[allow(clippy::cast_possible_truncation)]
            let level = v.as_f64().context("volume")?.round().clamp(0.0, 100.0) as i64;
            soap(
                host,
                RENDERING,
                "SetVolume",
                &[
                    ("InstanceID", "0"),
                    ("Channel", "Master"),
                    ("DesiredVolume", &level.to_string()),
                ],
            )
            .await?
        }
        (key, _) => bail!(moli_i18n::tr!("pilotes.sonos.ne_se_regle_pas", point = key)),
    };
    Ok(())
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let host = config
        .host
        .as_deref()
        .context(moli_i18n::tr!("pilotes.sonos.hote_requis"))?;
    let (udn, room, model) = describe(host)
        .await
        .with_context(|| format!("sonos {host}"))?;
    let id = ctx.device_id(&udn);
    let announcer = match &config.announce {
        Some(settings) => Some(announce::Announcer::new(
            host,
            &udn,
            settings.clone(),
            speaker_pin(host, ctx).await,
            ctx.media_publisher(),
        )),
        None => None,
    };
    ctx.upsert_device(device(ctx, &id, &room, &model, announcer.is_some()));
    ctx.ready();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut online = None;
    // A silence survives a restart (the hub keeps the last values).
    let mut quiet_until = match (ctx.current(&id, "quiet"), ctx.current(&id, "quiet_until")) {
        (Some(Value::Bool(true)), Some(Value::Int(until))) => u64::try_from(until).ok(),
        _ => None,
    }
    .filter(|u| *u > moli_core::now_ms());
    set_quiet(ctx, &id, quiet_until);
    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Ok(()) };
                if &*command.key == "announce" {
                    let result = if quiet_until.is_some_and(|u| u > moli_core::now_ms()) {
                        // Silenced: nothing said, nobody bothered with an error.
                        tracing::info!("announcement not said: announcements are silenced");
                        Ok(())
                    } else {
                        speak(announcer.as_ref(), &command.value)
                    };
                    command.reply(result);
                    continue;
                }
                if &*command.key == "quiet" {
                    let on = matches!(command.value, Value::Bool(true));
                    quiet_until = on.then(|| moli_core::now_ms() + u64::try_from(QUIET_FOR.as_millis()).unwrap_or(u64::MAX));
                    set_quiet(ctx, &id, quiet_until);
                    command.reply(Ok(()));
                    continue;
                }
                let result = apply(host, &command).await.map_err(|e| format!("{e:#}"));
                if result.is_ok() {
                    ctx.set_state(&id, &command.key, command.value.clone());
                }
                command.reply(result);
            }
            _ = tick.tick() => {
                if quiet_until.is_some_and(|u| u <= moli_core::now_ms()) {
                    // Forgotten on: the house speaks again.
                    quiet_until = None;
                    set_quiet(ctx, &id, None);
                }
                match poll(host, false).await {
                Ok(polled) => {
                    if online != Some(true) {
                        online = Some(true);
                        ctx.set_availability(&id, true);
                    }
                    for (key, value) in polled.values {
                        ctx.set_state(&id, key, value);
                    }
                }
                Err(e) => {
                    if online != Some(false) {
                        online = Some(false);
                        ctx.set_availability(&id, false);
                        tracing::debug!(host, error = %e, "sonos unreachable");
                    }
                }
                }
            }
        }
    }
}

/// The silence's two points (until: ms since epoch, 0 when not silenced).
fn set_quiet(ctx: &DriverCtx, id: &DeviceId, until: Option<u64>) {
    ctx.set_state(id, "quiet", Value::Bool(until.is_some()));
    let until = i64::try_from(until.unwrap_or(0)).unwrap_or(i64::MAX);
    ctx.set_state(id, "quiet_until", Value::Int(until));
}

/// Queues an announcement (it takes seconds: speech, then the clip) and
/// answers at once; a failure later is told to the house.
fn speak(announcer: Option<&Arc<announce::Announcer>>, value: &Value) -> Result<(), String> {
    let announcer =
        announcer.ok_or_else(|| moli_i18n::tr!("pilotes.sonos.annonces_non_configurees"))?;
    let Value::Text(text) = value else {
        return Err(moli_i18n::tr!("pilotes.sonos.annonce_texte"));
    };
    let text = announce::check_text(text)
        .map_err(|e| e.to_string())?
        .to_owned();
    announcer.enqueue(text)
}

/// The fingerprint of the speaker's local API certificate: pinned at first
/// contact, then required. Without it, announcements still work, unpinned.
async fn speaker_pin(host: &str, ctx: &DriverCtx) -> Option<String> {
    if let Some(pin) = ctx.secret(WS_CERT) {
        return Some(pin);
    }
    let probe = moli_net::PinnedHttps::new(host, 1443, None).ok()?;
    if let Err(e) = probe.handshake().await {
        tracing::warn!(host, error = %format!("{e:#}"), "sonos local API unreachable: announcements unpinned");
        return None;
    }
    let seen = probe.seen_fingerprint()?;
    if ctx.can_store_secrets()
        && let Err(e) = ctx.store_secret(WS_CERT, &seen).await
    {
        tracing::warn!(error = %e, "sonos certificate not stored");
    }
    Some(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_soap_and_escaped_didl() {
        let position = "<s:Envelope><s:Body><u:GetPositionInfoResponse><Track>1</Track><TrackMetaData>&lt;DIDL-Lite&gt;&lt;item&gt;&lt;dc:title&gt;Bohemian Rhapsody&lt;/dc:title&gt;&lt;dc:creator&gt;Queen &amp;amp; co&lt;/dc:creator&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;</TrackMetaData></u:GetPositionInfoResponse></s:Body></s:Envelope>";
        let meta = unescape(tag(position, "TrackMetaData").unwrap());
        assert_eq!(tag(&meta, "title"), Some("Bohemian Rhapsody"));
        assert_eq!(unescape(tag(&meta, "creator").unwrap()), "Queen & co");
        assert_eq!(
            tag("<CurrentVolume>23</CurrentVolume>", "CurrentVolume"),
            Some("23")
        );
    }

    #[test]
    fn reads_lengths_and_covers() {
        assert_eq!(seconds("0:03:43"), Some(223));
        assert_eq!(seconds("1:00:05"), Some(3605));
        assert_eq!(seconds("NOT_IMPLEMENTED"), None);
        assert_eq!(seconds(""), None);
        assert_eq!(
            cover("192.168.1.10", "/getaa?s=1&u=x"),
            "http://192.168.1.10:1400/getaa?s=1&u=x"
        );
        assert_eq!(cover("192.168.1.10", "http://a/b.jpg"), "http://a/b.jpg");
    }

    #[test]
    fn a_driver_is_one_player_or_the_whole_home() {
        let one: Config = serde_json::from_str(r#"{"host":"192.168.1.10"}"#).unwrap();
        assert!(!one.discover);
        let all: Config = serde_json::from_str(r#"{"discover":true}"#).unwrap();
        assert!(all.discover && all.host.is_none());
    }

    #[test]
    fn the_source_says_when_the_tv_plays() {
        assert_eq!(
            source("x-sonos-htastream:RINCON_000000000000001400:spdif"),
            "tv"
        );
        assert_eq!(
            source("x-rincon-stream:RINCON_000000000000001400"),
            "line_in"
        );
        assert_eq!(source("x-rincon:RINCON_000000000000001400"), "group");
        assert_eq!(source("x-sonos-spotify:spotify%3atrack%3a1"), "music");
        assert_eq!(source("x-sonos-vli:RINCON_X1400:1,airplay:abc"), "airplay");
        assert_eq!(
            source("x-sonos-vli:RINCON_X1400:3,bluetooth:abc"),
            "bluetooth"
        );
        assert_eq!(source("x-sonosapi-hls:hls%3ara.1?sid=204"), "radio");
        assert_eq!(source(" "), "none");
        assert!(SOURCES.contains(&source("anything")));
    }
}
