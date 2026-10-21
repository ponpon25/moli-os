//! What a room can be told beyond play, volume and mute: next and previous,
//! shuffle and repeat, joining and leaving a group, playing a favorite or
//! a saved playlist, the queue.

use anyhow::{Context as _, bail};
use moli_core::Value;
use moli_net::upnp::tag;
use moli_runtime::CommandRequest;

use crate::apple::{self, AppleMusic};
use crate::household::Room;
use crate::playback::{self, Item, REPEATS};
use crate::{AV, soap, soap_slow};

const CONTENT: (&str, &str) = (
    "/MediaServer/ContentDirectory/Control",
    "urn:schemas-upnp-org:service:ContentDirectory:1",
);
const ID: (&str, &str) = ("InstanceID", "0");

/// The entries of a Sonos list (`FV:2` favorites, `SQ:` saved playlists,
/// `Q:0` the queue), as many as `count`.
pub(crate) async fn browse(host: &str, object: &str, count: u32) -> anyhow::Result<Vec<Item>> {
    let count = count.to_string();
    let answer = soap(
        host,
        CONTENT,
        "Browse",
        &[
            ("ObjectID", object),
            ("BrowseFlag", "BrowseDirectChildren"),
            ("Filter", "*"),
            ("StartingIndex", "0"),
            ("RequestedCount", &count),
            ("SortCriteria", ""),
        ],
    )
    .await?;
    Ok(playback::items(&answer))
}

/// What a room can start by name: the favorites, then the saved playlists.
pub(crate) async fn library(host: &str) -> anyhow::Result<Vec<Item>> {
    let mut all = browse(host, "FV:2", 100).await?;
    all.extend(browse(host, "SQ:", 100).await.unwrap_or_default());
    Ok(all)
}

/// What an order did, beyond what the speaker now says: nothing more, a
/// queue changed (read again), or a search to show.
pub(crate) enum Outcome {
    Done,
    Queue,
    Results { query: String, json: String },
}

/// One order to `room`. Playback orders go to the leader of its group.
#[allow(clippy::too_many_lines)] // one arm per order, one entry per point
pub(crate) async fn act(
    room: &Room,
    rooms: &[Room],
    apple: Option<&AppleMusic>,
    user_token: Option<&str>,
    command: &CommandRequest,
) -> anyhow::Result<Outcome> {
    let leader = room.leader_host.as_str();
    match (&*command.key, &command.value) {
        ("skip", Value::Text(way)) => {
            let action = match &**way {
                "next" => "Next",
                "previous" => "Previous",
                other => bail!("skip {other}"),
            };
            soap(leader, AV, action, &[ID]).await?;
            Ok(Outcome::Done)
        }
        ("shuffle", Value::Bool(on)) => {
            let (_, repeat) = mode(leader).await?;
            set_mode(leader, *on, repeat).await?;
            Ok(Outcome::Done)
        }
        ("repeat", Value::Text(repeat)) => {
            let repeat = REPEATS
                .iter()
                .find(|r| **r == &**repeat)
                .with_context(|| format!("repeat {repeat}"))?;
            let (shuffle, _) = mode(leader).await?;
            set_mode(leader, shuffle, repeat).await?;
            Ok(Outcome::Done)
        }
        ("join", Value::Text(name)) => {
            let wanted = name.trim().to_lowercase();
            let target = rooms
                .iter()
                .find(|r| r.name.to_lowercase() == wanted)
                .with_context(|| moli_i18n::tr!("pilotes.sonos.piece_inconnue", room = name))?;
            if target.leader_uuid != room.leader_uuid {
                let uri = format!("x-rincon:{}", target.leader_uuid);
                soap(
                    &room.host,
                    AV,
                    "SetAVTransportURI",
                    &[ID, ("CurrentURI", &uri), ("CurrentURIMetaData", "")],
                )
                .await?;
            }
            Ok(Outcome::Done)
        }
        ("leave", Value::Bool(true)) => {
            soap(&room.host, AV, "BecomeCoordinatorOfStandaloneGroup", &[ID]).await?;
            Ok(Outcome::Done)
        }
        ("favorite", Value::Text(name)) => {
            let library = library(leader).await?;
            let item = playback::find(&library, name)
                .with_context(|| moli_i18n::tr!("pilotes.sonos.favori_inconnu", name = name))?;
            if item.uri.is_empty() {
                bail!(moli_i18n::tr!("pilotes.sonos.favori_injouable"));
            }
            if playback::through_queue(&item.uri) {
                soap(leader, AV, "RemoveAllTracksFromQueue", &[ID]).await?;
                soap_slow(
                    leader,
                    AV,
                    "AddURIToQueue",
                    &[
                        ID,
                        ("EnqueuedURI", &item.uri),
                        ("EnqueuedURIMetaData", &item.meta),
                        ("DesiredFirstTrackNumberEnqueued", "0"),
                        ("EnqueueAsNext", "1"),
                    ],
                )
                .await?;
                from_queue(leader, &room.leader_uuid).await?;
            } else {
                soap(
                    leader,
                    AV,
                    "SetAVTransportURI",
                    &[
                        ID,
                        ("CurrentURI", &item.uri),
                        ("CurrentURIMetaData", &item.meta),
                    ],
                )
                .await?;
            }
            soap(leader, AV, "Play", &[ID, ("Speed", "1")]).await?;
            Ok(Outcome::Queue)
        }
        ("queue_play", value) => {
            let number = value.as_f64().context("queue_play")?;
            if !(1.0..=100_000.0).contains(&number) {
                bail!("queue_play {number}");
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let number = (number.round() as u32).to_string();
            from_queue(leader, &room.leader_uuid).await?;
            soap(
                leader,
                AV,
                "Seek",
                &[ID, ("Unit", "TRACK_NR"), ("Target", &number)],
            )
            .await?;
            soap(leader, AV, "Play", &[ID, ("Speed", "1")]).await?;
            Ok(Outcome::Done)
        }
        ("search", Value::Text(term)) => {
            let apple = apple.context(moli_i18n::tr!("pilotes.sonos.apple_non_configure"))?;
            let hits = apple.search(term).await?;
            Ok(Outcome::Results {
                query: term.trim().to_owned(),
                json: serde_json::to_string(&hits)?,
            })
        }
        ("browse", Value::Text(what)) => {
            let apple = apple.context(moli_i18n::tr!("pilotes.sonos.apple_non_configure"))?;
            let token = user_token.context(moli_i18n::tr!("pilotes.sonos.apple_deconnecte"))?;
            let hits = apple.browse(what, token).await?;
            Ok(Outcome::Results {
                query: format!("browse:{what}"),
                json: serde_json::to_string(&hits)?,
            })
        }
        ("play", Value::Text(order)) => {
            play_catalog(room, apple, order).await?;
            Ok(Outcome::Queue)
        }
        ("queue_clear", Value::Bool(true)) => {
            soap(leader, AV, "RemoveAllTracksFromQueue", &[ID]).await?;
            Ok(Outcome::Queue)
        }
        (key, _) => bail!(moli_i18n::tr!("pilotes.sonos.ne_se_regle_pas", point = key)),
    }
}

/// A result of the Apple Music search, asked to play now, next, or at the
/// end of the queue.
#[derive(serde::Deserialize)]
struct Order {
    kind: String,
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    mode: String,
}

async fn play_catalog(room: &Room, apple: Option<&AppleMusic>, order: &str) -> anyhow::Result<()> {
    let apple = apple.context(moli_i18n::tr!("pilotes.sonos.apple_non_configure"))?;
    let order: Order = serde_json::from_str(order).context("play: not an order")?;
    let leader = room.leader_host.as_str();
    // The account the speakers use for Apple Music is learnt from what they
    // already hold from it (a favorite).
    let library = library(leader).await?;
    let account = apple::account_of(library.iter().map(|i| i.uri.as_str()))
        .context(moli_i18n::tr!("pilotes.sonos.apple_pas_de_compte"))?;
    // The songs of an artist, or the one thing asked for.
    let items = if order.kind == "artist" {
        let mut songs = Vec::new();
        for (id, title) in apple.top_songs(&order.id).await? {
            songs.push(apple::playable("song", &id, &title, &account)?);
        }
        if songs.is_empty() {
            bail!(moli_i18n::tr!("pilotes.sonos.aucun_titre"));
        }
        songs
    } else {
        vec![apple::playable(
            &order.kind,
            &order.id,
            &order.title,
            &account,
        )?]
    };
    // A station is not a list of tracks: it plays as it is.
    if let [item] = items.as_slice()
        && !item.queue
    {
        soap(
            leader,
            AV,
            "SetAVTransportURI",
            &[
                ID,
                ("CurrentURI", &item.uri),
                ("CurrentURIMetaData", &item.meta),
            ],
        )
        .await?;
        soap(leader, AV, "Play", &[ID, ("Speed", "1")]).await?;
        return Ok(());
    }
    if matches!(order.kind.as_str(), "artist" | "album" | "playlist") {
        // Many songs take longer than an order may wait: the speaker is
        // filled in the background, and the order answers at once.
        let (leader, uuid, mode) = (leader.to_owned(), room.leader_uuid.clone(), order.mode);
        tokio::spawn(async move {
            if let Err(e) = enqueue_all(&leader, &uuid, &mode, &items).await {
                tracing::warn!(error = %format!("{e:#}"), "sonos: a list could not be queued");
            }
        });
        return Ok(());
    }
    enqueue_all(leader, &room.leader_uuid, &order.mode, &items).await
}

/// Puts `items` in the queue of the group led by `leader` (`mode`: `now`
/// clears it and plays, `next` goes after the one playing, `add` at the end).
async fn enqueue_all(
    leader: &str,
    uuid: &str,
    mode: &str,
    items: &[apple::Playable],
) -> anyhow::Result<()> {
    let media = soap(leader, AV, "GetMediaInfo", &[ID]).await?;
    let in_queue = tag(&media, "CurrentURI")
        .is_some_and(|uri| moli_net::upnp::unescape(uri).starts_with("x-rincon-queue:"));
    let now = mode != "next" && mode != "add" || !in_queue;
    if now {
        soap(leader, AV, "RemoveAllTracksFromQueue", &[ID]).await?;
    }
    // Next: right after the one playing (one thing); else at the end (and
    // an artist's songs always, so they keep their order).
    let after_current = mode != "add" && items.len() == 1;
    for (i, item) in items.iter().enumerate() {
        soap_slow(
            leader,
            AV,
            "AddURIToQueue",
            &[
                ID,
                ("EnqueuedURI", &item.uri),
                ("EnqueuedURIMetaData", &item.meta),
                ("DesiredFirstTrackNumberEnqueued", "0"),
                ("EnqueueAsNext", if after_current { "1" } else { "0" }),
            ],
        )
        .await?;
        // The first song plays as soon as it is in, the others join it.
        if now && i == 0 {
            from_queue(leader, uuid).await?;
            soap(leader, AV, "Play", &[ID, ("Speed", "1")]).await?;
        }
    }
    Ok(())
}

/// Shuffle and repeat now.
async fn mode(host: &str) -> anyhow::Result<(bool, &'static str)> {
    let settings = soap(host, AV, "GetTransportSettings", &[ID]).await?;
    Ok(playback::mode_parts(
        tag(&settings, "PlayMode").unwrap_or("NORMAL"),
    ))
}

async fn set_mode(host: &str, shuffle: bool, repeat: &str) -> anyhow::Result<()> {
    soap(
        host,
        AV,
        "SetPlayMode",
        &[ID, ("NewPlayMode", playback::play_mode(shuffle, repeat))],
    )
    .await?;
    Ok(())
}

/// Plays from the queue of the group's leader.
async fn from_queue(host: &str, uuid: &str) -> anyhow::Result<()> {
    let uri = format!("x-rincon-queue:{uuid}#0");
    soap(
        host,
        AV,
        "SetAVTransportURI",
        &[ID, ("CurrentURI", &uri), ("CurrentURIMetaData", "")],
    )
    .await?;
    Ok(())
}
