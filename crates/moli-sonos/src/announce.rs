//! Spoken announcements: text → speech (a local Piper server) → a sound Moli
//! publishes for a few minutes → played by the speaker as an *audio clip*,
//! over what it was playing (the music dips, then comes back by itself).
//!
//! The audio clip goes through the speaker's local WebSocket API (port 1443,
//! the public key every local client uses): the household id, then the group
//! list to find this player, then `loadAudioClip`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use moli_net::PinnedTls;
use moli_net::ws::WebSocket;
use moli_runtime::MediaPublisher;
use moli_runtime::media::Image;
use serde::Deserialize;
use serde_json::{Value, json};

const WS_PORT: u16 = 1443;
const WS_PATH: &str = "/websocket/api";
/// The local API's public key (the same for every local client).
const API_KEY: &str = "123e4567-e89b-12d3-a456-426655440000";
const PROTOCOL: &str = "v1.api.smartspeaker.audio";
/// An announcement is a sentence or two, not a speech.
pub const MAX_TEXT: usize = 500;

/// `[driver.options.announce]`
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Moli as the speaker reaches it (`http://192.168.0.62:8790`): the
    /// speaker fetches the sound from there.
    pub media_base: String,
    /// The Wyoming speech server (Piper), `host:port`.
    #[serde(default = "default_tts")]
    pub tts: String,
    /// Piper voice (default: the server's).
    #[serde(default)]
    pub voice: Option<String>,
    /// Volume of the announcement, 0–100 (the speaker's own volume comes
    /// back afterwards). Home Assistant's script used 35.
    #[serde(default = "default_volume")]
    pub volume: u8,
    /// Volume of the door chime, 0–100 (default: the voice's).
    #[serde(default)]
    pub chime_volume: Option<u8>,
    /// With `discover`: only these rooms speak (empty: every room). Keeps
    /// the bedrooms silent by construction.
    #[serde(default)]
    pub rooms: Vec<String>,
}

/// How loud an announcement is: the voice and the chime, each 0–100.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levels {
    pub voice: u8,
    pub chime: u8,
}

impl Config {
    /// The levels of a room that was never set.
    #[must_use]
    pub fn levels(&self) -> Levels {
        Levels {
            voice: self.volume.min(100),
            chime: self.chime_volume.unwrap_or(self.volume).min(100),
        }
    }
}

fn default_tts() -> String {
    "127.0.0.1:10200".into()
}

fn default_volume() -> u8 {
    35
}

#[derive(Debug)]
pub struct Announcer {
    host: String,
    /// This player's id (`RINCON_…`, the UDN).
    player: String,
    config: Config,
    /// The speaker's certificate, pinned at first contact.
    pin: Option<String>,
    publisher: MediaPublisher,
    /// One announcement at a time, in order: each waits for the one before
    /// it to have played (its sound is made meanwhile, so a chime followed
    /// by a sentence leaves no gap).
    last: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Announcements waiting or playing.
    waiting: std::sync::atomic::AtomicUsize,
}

/// Announcements waiting at most: past it, a new one is refused (a burst
/// of alerts must not keep the speaker talking for minutes).
pub const MAX_WAITING: usize = 3;
/// An announcement that could not start within this delay is dropped: it
/// is no longer news.
pub const STALE_AFTER: Duration = Duration::from_secs(30);

impl Announcer {
    pub fn new(
        host: &str,
        player: &str,
        config: Config,
        pin: Option<String>,
        publisher: MediaPublisher,
    ) -> Arc<Self> {
        Arc::new(Self {
            host: host.to_owned(),
            player: player.to_owned(),
            config,
            pin,
            publisher,
            last: std::sync::Mutex::new(None),
            waiting: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Queues `text` and answers at once: the announcement itself takes
    /// seconds (speech, then the clip). Refused when too many are waiting;
    /// dropped if it could not start in time; the house is told of a failure.
    pub fn enqueue(self: &Arc<Self>, text: String, levels: Option<Levels>) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        let levels = levels.unwrap_or_else(|| self.config.levels());
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(moli_i18n::tr!(
                "pilotes.sonos.file_pleine",
                count = MAX_WAITING
            ));
        }
        let (played, next) = tokio::sync::oneshot::channel::<()>();
        let before = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(next);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let since = tokio::time::Instant::now();
            let sound = this.prepare(&text, levels).await;
            // The one before has played, failed or been dropped: our turn.
            if let Some(before) = before {
                let _ = before.await;
            }
            let outcome = match sound {
                Ok(_) if since.elapsed() > STALE_AFTER => Err(anyhow::anyhow!(moli_i18n::tr!(
                    "pilotes.sonos.trop_tard",
                    seconds = STALE_AFTER.as_secs()
                ))),
                Ok((url, volume)) => {
                    let ready = since.elapsed();
                    let result = this.play(&url, volume).await;
                    tracing::info!(
                        ready_ms = u64::try_from(ready.as_millis()).unwrap_or(u64::MAX),
                        total_ms = u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "announcement timing (queued, ready to play, played)"
                    );
                    result
                }
                Err(e) => Err(e),
            };
            drop(played);
            this.waiting.fetch_sub(1, Ordering::SeqCst);
            if let Err(e) = outcome {
                tracing::warn!(error = %format!("{e:#}"), "announcement not played");
                this.publisher.tell(
                    &moli_i18n::tr!("pilotes.sonos.annonce_vocale"),
                    moli_i18n::tr!(
                        "pilotes.sonos.non_annoncee",
                        text = short(&text),
                        error = format!("{e:#}")
                    ),
                );
            }
        });
        Ok(())
    }

    /// Makes the sound for `text` and publishes it: its address and the
    /// volume to play it at.
    async fn prepare(&self, text: &str, levels: Levels) -> anyhow::Result<(String, u8)> {
        let (tts_host, tts_port) = self
            .config
            .tts
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
            .context("tts must be host:port")?;
        let (chime, speech) = split_chime(text);
        let (wav, volume) = if speech.is_empty() {
            (chime_only(), levels.chime)
        } else {
            let wav = moli_net::wyoming::synthesize(
                tts_host,
                tts_port,
                self.config.voice.as_deref(),
                speech,
                Duration::from_secs(30),
            )
            .await?;
            mix(wav, chime, levels)
        };
        let name = self
            .publisher
            .publish(
                "wav",
                Image {
                    content_type: "audio/wav".into(),
                    bytes: wav,
                },
            )
            .context("no random name for the sound")?;
        let url = format!(
            "{}/api/media/{name}",
            self.config.media_base.trim_end_matches('/')
        );
        Ok((url, volume))
    }

    async fn play(&self, url: &str, volume: u8) -> anyhow::Result<()> {
        let began = tokio::time::Instant::now();
        let tls = PinnedTls::new(self.pin.clone())?;
        let mut ws = WebSocket::connect(
            &self.host,
            WS_PORT,
            WS_PATH,
            &[
                ("X-Sonos-Api-Key", API_KEY),
                ("Sec-WebSocket-Protocol", PROTOCOL),
            ],
            &tls,
        )
        .await?;
        let connected = began.elapsed();
        // An empty request: the speaker answers with an error, but its
        // header carries the household id (what sonos-websocket does too).
        let (header, _) = exchange(&mut ws, json!([{}, {}])).await?;
        let household = header
            .get("householdId")
            .and_then(Value::as_str)
            .context("no household id")?
            .to_owned();
        let (_, groups) = ask(
            &mut ws,
            json!([{ "namespace": "groups:1", "command": "getGroups", "householdId": household }, {}]),
        )
        .await?;
        let player = player_id(&groups, &self.player, &self.host)
            .context("this speaker is not in its household's player list")?;
        let known = began.elapsed();
        // The clip's fate (fetched, played, failed) comes as events.
        if let Err(e) = ask(
            &mut ws,
            json!([{ "namespace": "audioClip:1", "command": "subscribe", "playerId": player }, {}]),
        )
        .await
        {
            tracing::debug!(error = %format!("{e:#}"), "audio clip events not subscribed");
        }
        let (header, body) = ask(
            &mut ws,
            json!([
                { "namespace": "audioClip:1", "command": "loadAudioClip", "playerId": player },
                {
                    "name": "Moli",
                    "appId": "local.moli.os",
                    "streamUrl": url,
                    "volume": volume.min(100),
                }
            ]),
        )
        .await?;
        refused(&header, &body)?;
        tracing::info!(
            connected_ms = u64::try_from(connected.as_millis()).unwrap_or(u64::MAX),
            players_ms = u64::try_from(known.as_millis()).unwrap_or(u64::MAX),
            accepted_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
            clip = %body,
            "announcement accepted by the speaker"
        );
        follow(&mut ws).await
    }
}

/// Listens to the clip a little: a speaker that cannot fetch or play the
/// sound says so only there.
async fn follow(ws: &mut WebSocket) -> anyhow::Result<()> {
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        let Ok(text) = ws.recv_text(left).await else {
            return Ok(());
        };
        let event: Value = serde_json::from_str(&text).unwrap_or_default();
        let clips = event
            .get(1)
            .and_then(|b| b.get("audioClips"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for clip in &clips {
            let status = clip.get("status").and_then(Value::as_str).unwrap_or("");
            tracing::info!(status, clip = %clip, "announcement status");
            match status {
                "ERROR" => bail!(moli_i18n::tr!(
                    "pilotes.sonos.lecture_impossible",
                    reason = clip.get("errorCode").and_then(Value::as_str).map_or_else(
                        || moli_i18n::tr!("pilotes.sonos.sans_raison"),
                        str::to_owned
                    )
                )),
                "DONE" | "DISMISSED" => return Ok(()),
                _ => {}
            }
        }
    }
}

/// One request, one answer: `[header, body]`, a refusal being an error.
async fn ask(ws: &mut WebSocket, request: Value) -> anyhow::Result<(Value, Value)> {
    let (header, body) = exchange(ws, request).await?;
    refused(&header, &body)?;
    Ok((header, body))
}

/// One request, one answer, whatever it says.
async fn exchange(ws: &mut WebSocket, request: Value) -> anyhow::Result<(Value, Value)> {
    ws.send_text(&request.to_string()).await?;
    let answer: Value = serde_json::from_str(&ws.recv_text(Duration::from_secs(10)).await?)
        .context("the speaker's answer is not JSON")?;
    let header = answer.get(0).cloned().unwrap_or_default();
    let body = answer.get(1).cloned().unwrap_or_default();
    Ok((header, body))
}

/// The speaker's way of saying no.
fn refused(header: &Value, body: &Value) -> anyhow::Result<()> {
    let error = header.get("success") == Some(&Value::Bool(false))
        || body.get("_objectType").and_then(Value::as_str) == Some("globalError");
    if error {
        bail!(moli_i18n::tr!(
            "pilotes.sonos.refus_enceinte",
            reason = body
                .get("errorCode")
                .or_else(|| body.get("reason"))
                .and_then(Value::as_str)
                .map_or_else(
                    || moli_i18n::tr!("pilotes.sonos.erreur_inconnue"),
                    str::to_owned
                )
        ));
    }
    Ok(())
}

/// This player in `getGroups`: by id (the UDN), else by its websocket URL.
fn player_id(groups: &Value, udn: &str, host: &str) -> Option<String> {
    let players = groups.get("players")?.as_array()?;
    let url = format!("wss://{host}:{WS_PORT}{WS_PATH}");
    players
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(udn))
        .or_else(|| {
            players
                .iter()
                .find(|p| p.get("websocketUrl").and_then(Value::as_str) == Some(url.as_str()))
        })
        .and_then(|p| p.get("id").and_then(Value::as_str))
        .map(str::to_owned)
}

/// What an announcement may say.
/// The start of an announcement, for a message about it.
fn short(text: &str) -> String {
    let mut s: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        s.push('…');
    }
    s
}

/// A sentence that starts with this word is preceded by a door chime
/// (alone, it is the chime only).
pub const CHIME_MARK: &str = "[carillon]";

/// Whether the text asks for the chime, and what is left to say.
fn split_chime(text: &str) -> (bool, &str) {
    let text = text.trim();
    match text.get(..CHIME_MARK.len()) {
        Some(head) if head.eq_ignore_ascii_case(CHIME_MARK) => {
            (true, text[CHIME_MARK.len()..].trim())
        }
        _ => (false, text),
    }
}

/// Where the voice starts in a chime-and-voice sound: while the last note
/// is still ringing out, so the wait is short.
const VOICE_AT: f64 = 1.7;
/// Length of the chime: its three notes ring out about this long.
const CHIME_LEN: f64 = 2.6;

/// A struck tube: its partials (frequency ratio, level, decay per second)
/// are not harmonic, which is what makes a bell sound like a bell.
const TUBE: [(f64, f64, f64); 4] = [
    (1.0, 1.0, 2.2),
    (2.76, 0.45, 3.5),
    (5.40, 0.20, 6.0),
    (8.93, 0.08, 9.0),
];

/// The samples of a door chime: "ding-dang-dong", three notes going down
/// (G5, E5, C5), each a struck tube ringing out.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // audio samples, bounded
fn chime_pcm(rate: u32, gain: f64) -> Vec<i16> {
    let rate_f = f64::from(rate);
    let total = (rate_f * CHIME_LEN) as usize;
    let mut mix = vec![0f64; total];
    for (freq, start) in [(783.99_f64, 0.0_f64), (659.25, 0.45), (523.25, 0.9)] {
        let first = (start * rate_f) as usize;
        for (i, out) in mix.iter_mut().enumerate().skip(first) {
            let t = (i - first) as f64 / rate_f;
            let attack = (t / 0.002).min(1.0);
            let tone: f64 = TUBE
                .iter()
                .filter(|(ratio, _, _)| freq * ratio < rate_f / 2.2)
                .map(|(ratio, level, decay)| {
                    level * (std::f64::consts::TAU * freq * ratio * t).sin() * (-decay * t).exp()
                })
                .sum();
            *out += tone * attack;
        }
    }
    let peak = mix.iter().fold(0f64, |m, v| m.max(v.abs())).max(1e-9);
    mix.iter()
        .map(|v| (v / peak * 0.8 * gain * f64::from(i16::MAX)) as i16)
        .collect()
}

/// A WAV file of 16-bit PCM.
fn wav_file(rate: u32, channels: u16, pcm: &[i16]) -> Vec<u8> {
    let bytes = u32::try_from(pcm.len() * 2).unwrap_or(u32::MAX);
    let block = channels * 2;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&bytes.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// The format and PCM bytes of a 16-bit WAV file.
fn read_wav(wav: &[u8]) -> Option<(u32, u16, &[u8])> {
    if wav.get(..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut rate, mut channels, mut at) = (0u32, 0u16, 12usize);
    while let Some(head) = wav.get(at..at + 8) {
        let size = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
        let body = at + 8;
        match &head[..4] {
            b"fmt " => {
                let fmt = wav.get(body..body + 16)?;
                channels = u16::from_le_bytes(fmt[2..4].try_into().ok()?);
                rate = u32::from_le_bytes(fmt[4..8].try_into().ok()?);
                if u16::from_le_bytes(fmt[14..16].try_into().ok()?) != 16 {
                    return None;
                }
            }
            b"data" => {
                let end = body.saturating_add(size).min(wav.len());
                return (rate > 0 && channels > 0).then(|| (rate, channels, &wav[body..end]));
            }
            _ => {}
        }
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

/// The sound to play and the volume to play it at. The speaker has one
/// volume per clip: it is the louder of the two levels, and the other part
/// is made quieter in the sound itself. The speech alone when there is no
/// chime or its format is not the usual 16-bit PCM.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // audio samples, bounded
fn mix(speech: Vec<u8>, chime: bool, levels: Levels) -> (Vec<u8>, u8) {
    if !chime {
        return (speech, levels.voice);
    }
    let Some((rate, channels, pcm)) = read_wav(&speech) else {
        return (speech, levels.voice);
    };
    let top = levels.voice.max(levels.chime).max(1);
    let share = |level: u8| f64::from(level) / f64::from(top);
    let chime = chime_pcm(rate, share(levels.chime));
    let voice_gain = share(levels.voice);
    let voice: Vec<i16> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| (f64::from(i16::from_le_bytes(*b)) * voice_gain) as i16)
        .collect();
    let at = (f64::from(rate) * VOICE_AT) as usize;
    let mut mono = vec![0i32; chime.len().max(at + voice.len())];
    for (i, v) in chime.iter().enumerate() {
        mono[i] += i32::from(*v);
    }
    for (i, v) in voice.iter().enumerate() {
        mono[at + i] += i32::from(*v);
    }
    let mut samples: Vec<i16> = Vec::with_capacity(mono.len() * usize::from(channels));
    for v in mono {
        let v = v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        samples.extend(std::iter::repeat_n(v, usize::from(channels)));
    }
    (wav_file(rate, channels, &samples), top)
}

/// The chime alone (22.05 kHz mono, Piper's usual rate).
fn chime_only() -> Vec<u8> {
    wav_file(22_050, 1, &chime_pcm(22_050, 1.0))
}

pub fn check_text(text: &str) -> anyhow::Result<&str> {
    let text = text.trim();
    ensure!(
        !text.is_empty(),
        moli_i18n::tr!("pilotes.sonos.rien_a_dire")
    );
    ensure!(
        text.chars().count() <= MAX_TEXT,
        moli_i18n::tr!("pilotes.sonos.annonce_longueur", max = MAX_TEXT)
    );
    Ok(text)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;

    #[test]
    fn the_chime_comes_first_and_alone_is_a_sound() {
        assert_eq!(split_chime("[carillon] Bonjour"), (true, "Bonjour"));
        assert_eq!(split_chime("[Carillon]"), (true, ""));
        assert_eq!(
            split_chime("Bonjour [carillon]"),
            (false, "Bonjour [carillon]")
        );
        let speech = wav_file(22_050, 1, &vec![1000; 40_000]);
        let same = Levels {
            voice: 20,
            chime: 20,
        };
        let (both, volume) = mix(speech.clone(), true, same);
        assert_eq!(volume, 20);
        let (rate, channels, pcm) = read_wav(&both).unwrap();
        assert_eq!((rate, channels), (22_050, 1));
        let at = (22_050.0 * VOICE_AT) as usize;
        assert_eq!(
            pcm.len(),
            (at + 40_000) * 2,
            "the voice starts while the chime rings out"
        );
        // After the chime has ended only the voice is left.
        let end = (22_050.0 * CHIME_LEN) as usize + 10;
        assert_eq!(&pcm[end * 2..end * 2 + 2], &1000i16.to_le_bytes());
        assert!(read_wav(&chime_only()).is_some());
        let (plain, volume) = mix(b"not a wav".to_vec(), true, same);
        assert_eq!((plain.as_slice(), volume), (&b"not a wav"[..], 20));
        assert_eq!(mix(speech, false, same).1, 20);
    }

    #[test]
    fn voice_and_chime_have_their_own_level() {
        let speech = wav_file(22_050, 1, &vec![10_000; 40_000]);
        // The chime louder: the clip plays at the chime's level, the voice
        // is made quieter in the sound.
        let (wav, volume) = mix(
            speech.clone(),
            true,
            Levels {
                voice: 10,
                chime: 40,
            },
        );
        assert_eq!(volume, 40);
        let (_, _, pcm) = read_wav(&wav).unwrap();
        let end = (22_050.0 * CHIME_LEN) as usize + 10;
        let voice = i16::from_le_bytes([pcm[end * 2], pcm[end * 2 + 1]]);
        assert_eq!(voice, 2500);
        // The voice louder: the chime is made quieter.
        let (wav, volume) = mix(
            speech,
            true,
            Levels {
                voice: 40,
                chime: 10,
            },
        );
        assert_eq!(volume, 40);
        let (_, _, pcm) = read_wav(&wav).unwrap();
        let peak = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
            .take(20_000)
            .max()
            .unwrap();
        assert!(peak < 8_200, "{peak}");
    }

    #[test]
    fn finds_this_player_and_reads_refusals() {
        let groups = json!({ "players": [
            { "id": "RINCON_AAA01400", "websocketUrl": "wss://192.168.0.10:1443/websocket/api" },
            { "id": "RINCON_BBB01400", "websocketUrl": "wss://192.168.0.11:1443/websocket/api" }
        ]});
        assert_eq!(
            player_id(&groups, "RINCON_BBB01400", "x").as_deref(),
            Some("RINCON_BBB01400")
        );
        assert_eq!(
            player_id(&groups, "other", "192.168.0.10").as_deref(),
            Some("RINCON_AAA01400")
        );
        assert_eq!(player_id(&groups, "other", "192.168.0.99"), None);
        assert!(refused(&json!({"success": true}), &json!({})).is_ok());
        let no = refused(
            &json!({"success": false}),
            &json!({"_objectType": "globalError", "errorCode": "ERROR_INVALID_PARAMETER"}),
        );
        assert!(format!("{no:?}").contains("ERROR_INVALID_PARAMETER"));
        assert!(check_text("  ").is_err());
        assert!(check_text(&"a".repeat(MAX_TEXT + 1)).is_err());
        assert_eq!(check_text(" Quelqu'un sonne ").unwrap(), "Quelqu'un sonne");
    }
}
