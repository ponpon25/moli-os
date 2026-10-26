//! JointSpace pairing, done by the driver itself: a person asks for a code
//! (the `pairing` point, the TV's screen on), the TV shows it, the person
//! writes it in the `pin` point. The TV then trusts a device id and a key
//! of its choosing: they become the `username` and `password` secrets.

use anyhow::{Context as _, bail};
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_net::{Method, PinnedHttps};
use moli_runtime::DriverCtx;
use ring::hmac;
use ring::rand::{SecureRandom as _, SystemRandom};
use serde_json::{Value as Json, json};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

use crate::{CERT, Config, PASSWORD, PORT, Tv, USERNAME, first_pin};

/// The key every JointSpace client signs the code with (the same in every
/// TV and every client: it proves nothing, the TV wants it).
const SIGNING_KEY: [u8; 64] = [
    102, 101, 90, 203, 81, 16, 84, 83, 154, 102, 28, 16, 224, 171, 252, 215, 42, 75, 1, 147, 92,
    205, 95, 108, 27, 130, 164, 177, 229, 214, 159, 83, 68, 147, 167, 23, 152, 242, 142, 252, 192,
    154, 246, 202, 242, 178, 91, 203, 8, 83, 39, 53, 238, 25, 224, 35, 215, 205, 58, 40, 194, 248,
    71, 188,
];

/// How long a code lives, at most: the TV says 60 s in its answer but
/// counts 30 s down on screen, and answers « TIMEOUT » after.
const CODE_LIFE: u64 = 30;

/// A code asked for: what the TV gave to finish with.
struct Pending {
    device: Json,
    id: String,
    key: String,
    timestamp: Json,
    until: Instant,
}

/// Pairs the TV, then hands back its credentials (already stored).
/// `None`: the driver is stopping.
pub(crate) async fn pair(
    config: &Config,
    ctx: &mut DriverCtx,
) -> anyhow::Result<Option<(String, String)>> {
    let id = ctx.device_id(&config.host);
    ctx.upsert_device(device(ctx, &id));
    ctx.set_availability(&id, true);
    ctx.set_state(&id, "pairing", Value::Bool(false));
    ctx.wait_for(moli_i18n::tr!(
        "pilotes.philips.a_appairer",
        instance = ctx.instance()
    ));
    let mut pending: Option<Pending> = None;
    loop {
        let Some(command) = ctx.next_command().await else {
            return Ok(None);
        };
        let (key, value) = (command.key.clone(), command.value.clone());
        let result = match (&*key, &value) {
            ("pairing", Value::Bool(true)) => match request(config, ctx).await {
                Ok(asked) => {
                    let seconds = asked.until.saturating_duration_since(Instant::now());
                    ctx.wait_for(moli_i18n::tr!(
                        "pilotes.philips.code_affiche",
                        seconds = seconds.as_secs()
                    ));
                    pending = Some(asked);
                    Ok(())
                }
                Err(e) => Err(moli_i18n::tr!(
                    "pilotes.philips.appairage_refuse",
                    error = format!("{e:#}")
                )),
            },
            ("pairing", Value::Bool(false)) => {
                pending = None;
                Ok(())
            }
            ("pin", Value::Text(code)) => {
                let code = code.trim();
                if code.is_empty() || !code.bytes().all(|b| b.is_ascii_digit()) {
                    Err(moli_i18n::tr!("pilotes.philips.code_chiffres"))
                } else {
                    match pending.take() {
                        None => Err(moli_i18n::tr!("pilotes.philips.appairage_pas_ouvert")),
                        Some(asked) if Instant::now() >= asked.until => {
                            Err(moli_i18n::tr!("pilotes.philips.code_expire"))
                        }
                        Some(asked) => match grant(config, ctx, &asked, code).await {
                            Ok(()) => {
                                ctx.store_secret(USERNAME, &asked.id).await?;
                                ctx.store_secret(PASSWORD, &asked.key).await?;
                                tracing::info!(instance = %ctx.instance(), "tv paired");
                                command.reply(Ok(()));
                                return Ok(Some((asked.id, asked.key)));
                            }
                            Err(e) => {
                                // A wrong code: the person may type it again.
                                pending = Some(asked);
                                Err(moli_i18n::tr!(
                                    "pilotes.philips.appairage_refuse",
                                    error = format!("{e:#}")
                                ))
                            }
                        },
                    }
                }
            }
            ("pairing", _) => Err(moli_i18n::tr!("pilotes.philips.oui_non")),
            _ => Err(moli_i18n::tr!("pilotes.philips.pas_appairee")),
        };
        ctx.set_state(&id, "pairing", Value::Bool(pending.is_some()));
        command.reply(result);
    }
}

/// The TV, its certificate pinned (the first one seen if none yet).
async fn tv(
    config: &Config,
    ctx: &DriverCtx,
    username: &str,
    password: &str,
) -> anyhow::Result<Tv> {
    let pin = match ctx.secret(CERT) {
        Some(pin) => pin,
        None => first_pin(config, ctx).await?,
    };
    Ok(Tv {
        http: PinnedHttps::new(&config.host, PORT, Some(pin))?,
        api: config.api_version,
        username: username.into(),
        password: password.into(),
        auth: Mutex::new((None, 0)),
    })
}

/// Asks the TV for a code (it shows it on screen).
async fn request(config: &Config, ctx: &DriverCtx) -> anyhow::Result<Pending> {
    let id = device_id()?;
    let device = json!({
        "device_name": "Moli",
        "device_os": "Linux",
        "app_id": "app.moli",
        "app_name": "Moli",
        "type": "native",
        "id": id,
    });
    let body = json!({ "scope": ["read", "write", "control"], "device": device });
    let tv = tv(config, ctx, "", "").await?;
    // Once: a second request would show a second code.
    let answer = tv
        .call(Method::POST, "pair/request", Some(&body), true)
        .await?;
    succeeded(&answer)?;
    let key = answer["auth_key"]
        .as_str()
        .context("no auth_key in the TV's answer")?
        .to_owned();
    let life = answer["timeout"]
        .as_u64()
        .map_or(CODE_LIFE, |t| t.min(CODE_LIFE));
    Ok(Pending {
        device,
        id,
        key,
        timestamp: answer["timestamp"].clone(),
        until: Instant::now() + Duration::from_secs(life),
    })
}

/// Gives the TV its code back, signed: it then trusts `id` and `key`.
async fn grant(
    config: &Config,
    ctx: &DriverCtx,
    asked: &Pending,
    code: &str,
) -> anyhow::Result<()> {
    let timestamp = match &asked.timestamp {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    };
    let body = json!({
        "auth": {
            "auth_AppId": "1",
            "pin": code,
            "auth_timestamp": asked.timestamp,
            "auth_signature": signature(&timestamp, code),
        },
        "device": asked.device,
    });
    let tv = tv(config, ctx, &asked.id, &asked.key).await?;
    let answer = tv
        .call(Method::POST, "pair/grant", Some(&body), true)
        .await?;
    succeeded(&answer)
}

fn succeeded(answer: &Json) -> anyhow::Result<()> {
    match answer["error_id"].as_str() {
        None | Some("SUCCESS") => Ok(()),
        Some("TIMEOUT") => bail!(moli_i18n::tr!("pilotes.philips.code_expire")),
        Some(error) => bail!("{error}"),
    }
}

/// Base64 of the hex HMAC-SHA1 of timestamp then code.
fn signature(timestamp: &str, code: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &SIGNING_KEY);
    let tag = hmac::sign(&key, format!("{timestamp}{code}").as_bytes());
    let hex = tag
        .as_ref()
        .iter()
        .fold(String::with_capacity(40), |mut out, b| {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
            out
        });
    base64(hex.as_bytes())
}

/// Sixteen letters and digits: who Moli is for the TV.
fn device_id() -> anyhow::Result<String> {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("no randomness"))?;
    Ok(bytes
        .iter()
        .map(|b| char::from(LETTERS[usize::from(*b) % LETTERS.len()]))
        .collect())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The TV before pairing: only what pairs it.
fn device(ctx: &DriverCtx, id: &DeviceId) -> Device {
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: moli_i18n::tr!("pilotes.philips.tele").into(),
        manufacturer: Some("Philips".into()),
        model: Some("Android TV".into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![
            // Asks the TV for a code. No approval: only someone in front of
            // the screen sees the code that finishes it.
            PointSpec {
                key: "pairing".into(),
                label: moli_i18n::tr!("pilotes.philips.appairage_point").into(),
                kind: Kind::Binary,
                access: Access {
                    read: true,
                    write: true,
                },
                unit: None,
                semantic: Semantic::Other,
            },
            PointSpec {
                key: "pin".into(),
                label: moli_i18n::tr!("pilotes.philips.code_pin").into(),
                kind: Kind::Text,
                access: Access {
                    read: false,
                    write: true,
                },
                unit: None,
                semantic: Semantic::Other,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_like_other_clients() {
        // The signature other JointSpace clients compute for these values.
        assert_eq!(
            signature("12345", "1234"),
            "N2EzYWVmNDU0M2QxOWNjY2UxYjg2NTQ5N2YwODhkYjhlYjFlNzE2OA=="
        );
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn device_ids_are_sixteen_letters_or_digits() {
        let id = device_id().unwrap();
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric()));
    }
}
