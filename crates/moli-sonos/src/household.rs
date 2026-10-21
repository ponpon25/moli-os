//! Every Sonos player of the home as one device per room: found by SSDP,
//! then listed by the topology any player gives (`ZoneGroupTopology`). A
//! stereo pair or a home-theater set is one room (its other members are
//! invisible); rooms grouped together share what plays.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use moli_core::{Access, DeviceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_net::upnp::{Location, discover, tag, unescape};
use moli_runtime::DriverCtx;
use tokio::task::JoinSet;

use crate::actions::Outcome;
use crate::apple::AppleMusic;
use crate::cover::Cover;
use crate::events::{self, Listener};
use crate::playback::{self, REPEATS};
use crate::{Config, POLL, PORT, Polled, actions, apply, describe, device, poll, soap};

const TOPOLOGY: (&str, &str) = (
    "/ZoneGroupTopology/Control",
    "urn:schemas-upnp-org:service:ZoneGroupTopology:1",
);
const PLAYER: &str = "urn:schemas-upnp-org:device:ZonePlayer:1";
/// What a group's followers take from the group's leader.
const SHARED: [&str; 12] = [
    "playing",
    "state",
    "title",
    "artist",
    "album",
    "art",
    "duration",
    "position",
    "shuffle",
    "repeat",
    "queue_position",
    "queue_length",
];
/// The favorites and a queue are read again after this, whatever happened.
const LIBRARY_AGE: Duration = Duration::from_secs(300);
/// With events, a player not heard from is read anyway this often.
const SAFETY: Duration = Duration::from_secs(10);
/// A subscription's life; renewed at half of it.
const LEASE: u32 = 300;
/// A subscription that failed is tried again after this.
const RETRY: Duration = Duration::from_secs(30);
/// Events come in bursts: read once, a moment after the last one.
const SETTLE: Duration = Duration::from_millis(250);
/// A position the room can follow by itself (its clock) is not told again
/// unless it drifts by more than this many seconds.
const DRIFT: i64 = 3;
/// What is subscribed to on each player: playback, volume.
const SERVICES: [(&str, &str); 2] = [
    ("av", "/MediaRenderer/AVTransport/Event"),
    ("rc", "/MediaRenderer/RenderingControl/Event"),
];

/// One player of a group, as the topology lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Member {
    uuid: String,
    name: String,
    host: String,
    /// Not shown: the other half of a stereo pair, a sub, a surround.
    hidden: bool,
    /// Part of a stereo pair (the two halves are one room).
    pair: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    coordinator: String,
    members: Vec<Member>,
}

/// A room: what the house sees as one speaker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Room {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) host: String,
    pair: bool,
    /// The room that plays for the group, and where to send its orders.
    leader: String,
    pub(crate) leader_host: String,
    /// The player that leads the group (what a room joins to join it).
    pub(crate) leader_uuid: String,
    /// The other rooms of the group.
    with: Vec<String>,
}

/// The value of `name="…"` in the opening of a tag.
fn attr(head: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let mut from = 0;
    while let Some(i) = head[from..].find(&key) {
        let at = from + i;
        if at == 0 || head.as_bytes()[at - 1] == b' ' {
            let rest = &head[at + key.len()..];
            return Some(unescape(&rest[..rest.find('"')?]));
        }
        from = at + key.len();
    }
    None
}

/// The groups of a `GetZoneGroupState` answer.
fn parse_topology(response: &str) -> Vec<Group> {
    let Some(state) = tag(response, "ZoneGroupState") else {
        return Vec::new();
    };
    let xml = unescape(state);
    xml.split("<ZoneGroup ")
        .skip(1)
        .filter_map(|group| {
            let coordinator = attr(group.split('>').next()?, "Coordinator")?;
            let body = group.split("</ZoneGroup>").next()?;
            let members = body
                .split("<ZoneGroupMember ")
                .skip(1)
                .filter_map(|member| {
                    let head = member.split('>').next()?;
                    Some(Member {
                        uuid: attr(head, "UUID")?,
                        name: attr(head, "ZoneName")?,
                        host: Location::parse(&attr(head, "Location")?)?.host,
                        hidden: attr(head, "Invisible").as_deref() == Some("1"),
                        pair: attr(head, "ChannelMapSet").is_some_and(|map| map.contains(';')),
                    })
                })
                .collect();
            Some(Group {
                coordinator,
                members,
            })
        })
        .collect()
}

/// The rooms of the groups, and the players that are not rooms.
fn rooms(groups: &[Group]) -> (Vec<Room>, HashSet<String>) {
    let mut rooms = Vec::new();
    let mut hidden = HashSet::new();
    for group in groups {
        let shown: Vec<&Member> = group.members.iter().filter(|m| !m.hidden).collect();
        hidden.extend(
            group
                .members
                .iter()
                .filter(|m| m.hidden)
                .map(|m| m.uuid.clone()),
        );
        // The leader first; a leader that is not shown (a pair's second half)
        // is answered for by the first room shown.
        let Some(leader) = shown
            .iter()
            .find(|m| m.uuid == group.coordinator)
            .or_else(|| shown.first())
        else {
            continue;
        };
        let leader_host = group
            .members
            .iter()
            .find(|m| m.uuid == group.coordinator)
            .map_or_else(|| leader.host.clone(), |m| m.host.clone());
        for member in &shown {
            rooms.push(Room {
                uuid: member.uuid.clone(),
                name: member.name.clone(),
                host: member.host.clone(),
                pair: member.pair,
                leader: leader.name.clone(),
                leader_host: leader_host.clone(),
                leader_uuid: group.coordinator.clone(),
                with: shown
                    .iter()
                    .filter(|other| other.uuid != member.uuid)
                    .map(|other| other.name.clone())
                    .collect(),
            });
        }
    }
    (rooms, hidden)
}

/// Where an order goes: playback to the group's leader, the rest to the
/// room itself (volume and mute are the room's own).
fn target<'a>(room: &'a Room, key: &str) -> &'a str {
    if key == "playing" {
        &room.leader_host
    } else {
        &room.host
    }
}

fn extra(key: &str, label: String, kind: Kind) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access {
            read: true,
            write: false,
        },
        unit: None,
        semantic: Semantic::Other,
    }
}

/// A point the house can read and set.
fn control(key: &str, label: String, kind: Kind) -> PointSpec {
    PointSpec {
        access: Access {
            read: true,
            write: true,
        },
        ..extra(key, label, kind)
    }
}

/// A point that is an order, not a state.
fn press(key: &str, label: String, kind: Kind) -> PointSpec {
    PointSpec {
        access: Access {
            read: false,
            write: true,
        },
        ..extra(key, label, kind)
    }
}

/// The device of a room: a speaker, and what it is grouped with.
#[allow(clippy::too_many_lines)] // one arm per order, one entry per point
fn room_device(
    ctx: &DriverCtx,
    id: &DeviceId,
    room: &Room,
    model: &str,
    apple: bool,
) -> moli_core::Device {
    let mut device = device(ctx, id, &room.name, model, false);
    if apple {
        device.points.push(extra(
            "results",
            moli_i18n::tr!("pilotes.sonos.resultats"),
            Kind::Text,
        ));
        device.points.push(extra(
            "results_for",
            moli_i18n::tr!("pilotes.sonos.resultats_pour"),
            Kind::Text,
        ));
        device.points.push(press(
            "search",
            moli_i18n::tr!("pilotes.sonos.recherche"),
            Kind::Text,
        ));
        device.points.push(press(
            "browse",
            moli_i18n::tr!("pilotes.sonos.parcourir_apple"),
            Kind::Text,
        ));
        device.points.push(press(
            "play",
            moli_i18n::tr!("pilotes.sonos.lire_apple"),
            Kind::Text,
        ));
    }
    device.points.push(extra(
        "group_leader",
        moli_i18n::tr!("pilotes.sonos.groupe_chef"),
        Kind::Text,
    ));
    device.points.push(extra(
        "group_with",
        moli_i18n::tr!("pilotes.sonos.groupe_avec"),
        Kind::Text,
    ));
    device.points.push(extra(
        "group_size",
        moli_i18n::tr!("pilotes.sonos.groupe_taille"),
        Kind::Numeric {
            min: Some(1.0),
            max: None,
            step: Some(1.0),
        },
    ));
    device.points.push(extra(
        "album",
        moli_i18n::tr!("pilotes.sonos.album"),
        Kind::Text,
    ));
    device.points.push(extra(
        "art",
        moli_i18n::tr!("pilotes.sonos.pochette"),
        Kind::Text,
    ));
    for (key, label) in [
        ("duration", moli_i18n::tr!("pilotes.sonos.duree")),
        ("position", moli_i18n::tr!("pilotes.sonos.position")),
    ] {
        let mut point = extra(
            key,
            label,
            Kind::Numeric {
                min: Some(0.0),
                max: None,
                step: Some(1.0),
            },
        );
        point.unit = Some(Unit::Second);
        device.points.push(point);
    }
    device.points.push(control(
        "shuffle",
        moli_i18n::tr!("pilotes.sonos.aleatoire"),
        Kind::Binary,
    ));
    device.points.push(control(
        "repeat",
        moli_i18n::tr!("pilotes.sonos.repetition"),
        Kind::Enum {
            values: REPEATS.iter().map(|r| (*r).into()).collect(),
        },
    ));
    for (key, label) in [
        (
            "queue_position",
            moli_i18n::tr!("pilotes.sonos.file_position"),
        ),
        ("queue_length", moli_i18n::tr!("pilotes.sonos.file_taille")),
    ] {
        device.points.push(extra(
            key,
            label,
            Kind::Numeric {
                min: Some(1.0),
                max: None,
                step: Some(1.0),
            },
        ));
    }
    device.points.push(extra(
        "queue",
        moli_i18n::tr!("pilotes.sonos.file"),
        Kind::Text,
    ));
    device.points.push(extra(
        "favorites",
        moli_i18n::tr!("pilotes.sonos.favoris"),
        Kind::Text,
    ));
    // Presses and requests: nothing to read back.
    device.points.push(press(
        "skip",
        moli_i18n::tr!("pilotes.sonos.piste"),
        Kind::Enum {
            values: vec!["next".into(), "previous".into()],
        },
    ));
    device.points.push(press(
        "join",
        moli_i18n::tr!("pilotes.sonos.rejoindre"),
        Kind::Text,
    ));
    device.points.push(press(
        "leave",
        moli_i18n::tr!("pilotes.sonos.quitter"),
        Kind::Binary,
    ));
    device.points.push(press(
        "favorite",
        moli_i18n::tr!("pilotes.sonos.lancer_favori"),
        Kind::Text,
    ));
    device.points.push(press(
        "queue_play",
        moli_i18n::tr!("pilotes.sonos.lire_numero"),
        Kind::Numeric {
            min: Some(1.0),
            max: None,
            step: Some(1.0),
        },
    ));
    device.points.push(press(
        "queue_clear",
        moli_i18n::tr!("pilotes.sonos.vider_file"),
        Kind::Binary,
    ));
    if room.pair {
        device.description = Some(moli_i18n::tr!("pilotes.sonos.paire_stereo").into());
    }
    device
}

/// The players that answer a search, by address.
async fn found() -> Vec<String> {
    match discover(PLAYER, Duration::from_secs(3)).await {
        Ok(found) => found
            .into_iter()
            .filter(|at| at.port == PORT)
            .map(|at| at.host)
            .collect(),
        Err(e) => {
            tracing::debug!(error = %e, "sonos search failed");
            Vec::new()
        }
    }
}

/// The topology, from the first player that gives it (the one that did
/// last time first).
async fn topology(hosts: &[String]) -> Option<(String, Vec<Group>)> {
    for host in hosts {
        let Ok(answer) = soap(host, TOPOLOGY, "GetZoneGroupState", &[]).await else {
            continue;
        };
        let groups = parse_topology(&answer);
        if !groups.is_empty() {
            return Some((host.clone(), groups));
        }
    }
    None
}

/// What the house has published for a room.
struct Shown {
    room: Room,
    model: String,
    /// Where the room's cover is read (its group's leader).
    leader: Arc<Mutex<String>>,
}

/// The last reading of a room's player.
struct Reading {
    values: Vec<(&'static str, Value)>,
    at: Instant,
    playing: bool,
    track: String,
    /// The last position told, and when: the room follows it by its clock.
    told: Option<(i64, Instant)>,
}

/// Whether a position is worth telling: a new track or state, or a jump
/// from where the clock of a room playing since the last one would be.
fn position_moved(prev: Option<&Reading>, now: i64, playing: bool, track: &str) -> bool {
    let Some(prev) = prev else { return true };
    if prev.track != track || prev.playing != playing {
        return true;
    }
    let Some((was, at)) = prev.told else {
        return true;
    };
    let elapsed = if playing {
        i64::try_from(at.elapsed().as_secs()).unwrap_or(i64::MAX)
    } else {
        0
    };
    (now - (was + elapsed)).abs() > DRIFT
}

/// A subscription to a player's events.
struct Subscription {
    /// Empty: not subscribed (the last try failed).
    sid: String,
    renew_at: Instant,
}

/// A group's queue, as last read.
struct Queue {
    length: i64,
    at: Instant,
    json: String,
}

#[derive(Default)]
struct Home {
    shown: HashMap<DeviceId, Shown>,
    last: HashMap<String, Reading>,
    subscriptions: HashMap<(String, &'static str), Subscription>,
    /// By the group's leader.
    queues: HashMap<String, Queue>,
    /// The names of the favorites and saved playlists, as a point says them.
    library: Option<(Instant, String)>,
    /// Whether Apple Music is set up (its points are on the rooms).
    apple: bool,
}

/// The next player to call back (never, without a listener).
async fn next_kick(listener: &mut Option<Listener>) -> Option<String> {
    let kick = match listener {
        Some(listener) => listener.kicks.recv().await,
        None => std::future::pending::<Option<String>>().await,
    };
    if kick.is_none() {
        // The listener is gone: the rounds read everything again.
        *listener = None;
    }
    kick
}

async fn settled(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

/// The rooms to read for these players: them and the rest of their group
/// (what a follower shows comes from its leader).
fn with_group(home: &Home, who: &HashSet<String>) -> HashSet<String> {
    let rooms: Vec<&Room> = home.shown.values().map(|known| &known.room).collect();
    let hosts: HashSet<&str> = rooms
        .iter()
        .filter(|room| who.contains(&room.uuid))
        .map(|room| room.leader_host.as_str())
        .collect();
    rooms
        .iter()
        .filter(|room| hosts.contains(room.leader_host.as_str()))
        .map(|room| room.uuid.clone())
        .collect()
}

#[allow(clippy::too_many_lines)] // one arm per order, one entry per point
pub(crate) async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if config.announce.is_some() {
        bail!(moli_i18n::tr!("pilotes.sonos.annonces_decouverte"));
    }
    let mut seeds: Vec<String> = config.host.iter().cloned().collect();
    for host in found().await {
        if !seeds.contains(&host) {
            seeds.push(host);
        }
    }
    let (mut anchor, mut groups) = topology(&seeds)
        .await
        .context(moli_i18n::tr!("pilotes.sonos.aucune_enceinte"))?;
    ctx.ready();
    let mut listener = if config.events {
        match events::listen(config.events_port).await {
            Ok(listener) => Some(listener),
            Err(e) => {
                tracing::warn!(port = config.events_port, error = %e, "sonos events unavailable: reading at every round");
                None
            }
        }
    } else {
        None
    };
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut home = Home::default();
    let apple = apple_music(config, ctx);
    if let Some(settings) = &config.apple {
        file_identifiers(ctx, settings).await;
    }
    home.apple = apple.is_some();
    let mut dirty: HashSet<String> = HashSet::new();
    let mut settle: Option<tokio::time::Instant> = None;
    let mut lost = 0u32;
    let mut first = true;
    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Ok(()) };
                let ask = home.shown.get(&command.device.id).map(|known| known.room.clone());
                let result = match ask {
                    Some(room) if matches!(&*command.key, "playing" | "mute" | "volume") => {
                        apply(target(&room, &command.key), &command)
                            .await
                            .map_err(|e| format!("{e:#}"))
                    }
                    Some(room) => {
                        let rooms: Vec<Room> =
                            home.shown.values().map(|known| known.room.clone()).collect();
                        let user = ctx.secret("apple_music_user_token");
                        let done = actions::act(&room, &rooms, apple.as_deref(), user.as_deref(), &command)
                            .await
                            .map_err(|e| format!("{e:#}"));
                        match &done {
                            Ok(Outcome::Results { query, json }) => {
                                // What was found, for whoever asked.
                                let id = &command.device.id;
                                ctx.set_state(id, "results", Value::Text(json.as_str().into()));
                                ctx.set_state(id, "results_for", Value::Text(query.as_str().into()));
                            }
                            Ok(other) => {
                                if matches!(other, Outcome::Queue) {
                                    // A new queue: read it at once.
                                    home.queues.remove(&room.leader_uuid);
                                }
                                for uuid in [&room.uuid, &room.leader_uuid] {
                                    dirty.insert(uuid.clone());
                                }
                                settle.get_or_insert(tokio::time::Instant::now() + SETTLE);
                            }
                            Err(_) => {}
                        }
                        done.map(|_| ())
                    }
                    None => Err(moli_i18n::tr!("pilotes.sonos.enceinte_inconnue")),
                };
                let readable = command
                    .device
                    .points
                    .iter()
                    .any(|p| *p.key == *command.key && p.access.read);
                if result.is_ok() && readable {
                    ctx.set_state(&command.device.id, &command.key, command.value.clone());
                }
                command.reply(result);
            }
            who = next_kick(&mut listener) => {
                if let Some(who) = who {
                    tracing::debug!(player = %who, "sonos event");
                    dirty.insert(who);
                    settle.get_or_insert(tokio::time::Instant::now() + SETTLE);
                }
            }
            () = settled(settle) => {
                settle = None;
                let wanted = with_group(&home, &std::mem::take(&mut dirty));
                refresh(ctx, &groups, &mut home, Some(&wanted)).await;
            }
            _ = tick.tick() => {
                if first {
                    first = false;
                } else {
                    let mut hosts = vec![anchor.clone()];
                    for host in seeds.iter().chain(home.shown.values().map(|k| &k.room.host)) {
                        if !hosts.contains(host) {
                            hosts.push(host.clone());
                        }
                    }
                    if let Some(read) = topology(&hosts).await {
                        (anchor, groups) = read;
                        lost = 0;
                    } else {
                        lost += 1;
                        // Everything is out of reach: say so, and look again
                        // now and then (an address may have changed).
                        for id in home.shown.keys() {
                            ctx.set_availability(id, false);
                        }
                        if lost % 15 == 1 {
                            for host in found().await {
                                if !seeds.contains(&host) {
                                    seeds.push(host);
                                }
                            }
                        }
                        continue;
                    }
                }
                if home.library.as_ref().is_none_or(|(at, _)| at.elapsed() >= LIBRARY_AGE) {
                    let names = actions::library(&anchor)
                        .await
                        .map(|items| playback::names_json(&items));
                    home.library = Some((Instant::now(), names.unwrap_or_else(|_| "[]".into())));
                }
                let max_age = if listener.is_some() { SAFETY } else { Duration::ZERO };
                refresh_due(ctx, &groups, &mut home, max_age).await;
                if let Some(listener) = &listener {
                    keep(&mut home, listener.port).await;
                }
            }
        }
    }
}

/// The house's Apple identifiers (not secrets), filed with the key so the
/// dashboard can ask for a token to sign in to Apple with.
async fn file_identifiers(ctx: &DriverCtx, settings: &crate::apple::Config) {
    if !ctx.can_store_secrets() {
        return;
    }
    for (name, value) in [
        ("apple_team_id", &settings.team_id),
        ("apple_key_id", &settings.key_id),
        ("apple_storefront", &settings.storefront),
    ] {
        if ctx.secret(name).as_deref() != Some(value.as_str())
            && let Err(e) = ctx.store_secret(name, value).await
        {
            tracing::warn!(error = %e, "sonos: apple identifier not filed");
        }
    }
}

/// Apple Music, when the house set it up and gave the key.
fn apple_music(config: &Config, ctx: &DriverCtx) -> Option<Arc<AppleMusic>> {
    let settings = config.apple.clone()?;
    let Some(pem) = ctx.secret("apple_music_key") else {
        tracing::warn!(
            "sonos: apple music is set up but its key (apple_music_key) is not in the vault"
        );
        return None;
    };
    match AppleMusic::new(settings, &pem) {
        Ok(apple) => Some(Arc::new(apple)),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "sonos: apple music key refused");
            None
        }
    }
}

/// The rooms not read for `max_age`.
async fn refresh_due(ctx: &DriverCtx, groups: &[Group], home: &mut Home, max_age: Duration) {
    let (rooms, _) = rooms(groups);
    let due: HashSet<String> = rooms
        .iter()
        .filter(|room| {
            home.last
                .get(&room.uuid)
                .is_none_or(|read| read.at.elapsed() >= max_age)
        })
        .map(|room| room.uuid.clone())
        .collect();
    refresh(ctx, groups, home, Some(&due)).await;
}

/// Subscribes to each player's events, and renews before they lapse.
async fn keep(home: &mut Home, port: u16) {
    let now = Instant::now();
    let mut jobs = JoinSet::new();
    for known in home.shown.values() {
        let room = &known.room;
        for (service, path) in SERVICES {
            let key = (room.uuid.clone(), service);
            let current = home.subscriptions.get(&key);
            if current.is_some_and(|s| s.renew_at > now) {
                continue;
            }
            let sid = current.map(|s| s.sid.clone()).filter(|s| !s.is_empty());
            let Some(callback) = events::callback(&room.host, port, &room.uuid, service) else {
                continue;
            };
            let host = room.host.clone();
            jobs.spawn(async move {
                let mut sid = match sid {
                    Some(sid) => {
                        moli_net::upnp::subscribe(&host, PORT, path, None, Some(&sid), LEASE)
                            .await
                            .ok()
                    }
                    None => None,
                };
                if sid.is_none() {
                    sid =
                        moli_net::upnp::subscribe(&host, PORT, path, Some(&callback), None, LEASE)
                            .await
                            .ok();
                }
                (key, sid)
            });
        }
    }
    while let Some(Ok((key, sid))) = jobs.join_next().await {
        let subscription = match sid {
            Some(sid) => Subscription {
                sid,
                renew_at: now + Duration::from_secs(u64::from(LEASE) / 2),
            },
            None => Subscription {
                sid: String::new(),
                renew_at: now + RETRY,
            },
        };
        home.subscriptions.insert(key, subscription);
    }
    let shown = &home.shown;
    home.subscriptions
        .retain(|(uuid, _), _| shown.values().any(|known| &known.room.uuid == uuid));
}

/// Publishes the rooms, reads `only` of them (all, when `None`), and tells
/// the groups.
#[allow(clippy::too_many_lines)] // one arm per order, one entry per point
async fn refresh(
    ctx: &DriverCtx,
    groups: &[Group],
    home: &mut Home,
    only: Option<&HashSet<String>>,
) {
    let (rooms, hidden) = rooms(groups);
    // A player that became half of a pair is no longer a speaker of its own.
    for uuid in &hidden {
        let id = ctx.device_id(uuid);
        if home.shown.remove(&id).is_some() {
            ctx.remove_device(&id);
        }
        home.last.remove(uuid);
    }
    for room in &rooms {
        let id = ctx.device_id(&room.uuid);
        let model = match home.shown.get(&id) {
            Some(known) => known.model.clone(),
            None => describe(&room.host)
                .await
                .map_or_else(|_| "Sonos".to_owned(), |(_, _, model)| model),
        };
        let changed = home
            .shown
            .get(&id)
            .is_none_or(|known| known.room.name != room.name || known.room.pair != room.pair);
        let leader = home.shown.get(&id).map_or_else(
            || Arc::new(Mutex::new(String::new())),
            |known| Arc::clone(&known.leader),
        );
        leader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&room.leader_host);
        if changed {
            ctx.upsert_device(room_device(ctx, &id, room, &model, home.apple));
            ctx.provide_cover(
                &id,
                Arc::new(Cover {
                    host: Arc::clone(&leader),
                }),
            );
        }
        home.shown.insert(
            id,
            Shown {
                room: room.clone(),
                model,
                leader,
            },
        );
    }
    let mut reads = JoinSet::new();
    for room in rooms
        .iter()
        .filter(|r| only.is_none_or(|o| o.contains(&r.uuid)))
    {
        let (uuid, host) = (room.uuid.clone(), room.host.clone());
        reads.spawn(async move { (uuid, poll(&host, true).await) });
    }
    while let Some(Ok((uuid, read))) = reads.join_next().await {
        match read {
            Ok(polled) => {
                let reading = reading(home.last.get(&uuid), polled);
                home.last.insert(uuid, reading);
            }
            Err(_) => {
                home.last.remove(&uuid);
            }
        }
    }
    for room in rooms.iter().filter(|r| r.host == r.leader_host) {
        let Some(read) = home.last.get(&room.uuid) else {
            continue;
        };
        let length = read
            .values
            .iter()
            .find_map(|(key, value)| match value {
                Value::Int(n) if *key == "queue_length" => Some(*n),
                _ => None,
            })
            .unwrap_or(0);
        let stale = home
            .queues
            .get(&room.uuid)
            .is_none_or(|q| q.length != length || q.at.elapsed() >= LIBRARY_AGE);
        if stale {
            let json = if length == 0 {
                Ok("[]".to_owned())
            } else {
                actions::browse(&room.host, "Q:0", 100)
                    .await
                    .map(|items| playback::queue_json(&items))
            };
            if let Ok(json) = json {
                home.queues.insert(
                    room.uuid.clone(),
                    Queue {
                        length,
                        at: Instant::now(),
                        json,
                    },
                );
            }
        }
    }
    let listed: HashSet<DeviceId> = rooms.iter().map(|r| ctx.device_id(&r.uuid)).collect();
    for id in home.shown.keys().filter(|id| !listed.contains(*id)) {
        ctx.set_availability(id, false);
    }
    for room in &rooms {
        let id = ctx.device_id(&room.uuid);
        let Some(own) = home.last.get(&room.uuid) else {
            ctx.set_availability(&id, false);
            continue;
        };
        ctx.set_availability(&id, true);
        let leader = rooms
            .iter()
            .find(|r| r.host == room.leader_host && r.uuid != room.uuid)
            .and_then(|r| home.last.get(&r.uuid));
        for (key, value) in &own.values {
            match leader.filter(|_| SHARED.contains(key)) {
                // What a follower shows is its leader's, or nothing new.
                Some(leader) => {
                    if let Some((_, v)) = leader.values.iter().find(|(k, _)| k == key) {
                        ctx.set_state(&id, key, v.clone());
                    }
                }
                None => ctx.set_state(&id, key, value.clone()),
            }
        }
        if let Some(queue) = home.queues.get(&room.leader_uuid) {
            ctx.set_state(&id, "queue", Value::Text(queue.json.as_str().into()));
        }
        if let Some((_, names)) = &home.library {
            ctx.set_state(&id, "favorites", Value::Text(names.as_str().into()));
        }
        ctx.set_state(
            &id,
            "group_leader",
            Value::Text(room.leader.as_str().into()),
        );
        ctx.set_state(
            &id,
            "group_with",
            if room.with.is_empty() {
                Value::Null
            } else {
                Value::Text(room.with.join(", ").into())
            },
        );
        ctx.set_state(
            &id,
            "group_size",
            Value::Int(i64::try_from(room.with.len() + 1).unwrap_or(i64::MAX)),
        );
    }
}

/// A new reading, after the previous one: the position only when it moved.
fn reading(prev: Option<&Reading>, polled: Polled) -> Reading {
    let playing = polled
        .values
        .iter()
        .any(|(key, value)| *key == "playing" && matches!(value, Value::Bool(true)));
    let mut values = polled.values;
    let mut told = prev.and_then(|p| p.told);
    if let Some(at) = polled.position {
        if position_moved(prev, at, playing, &polled.track) {
            values.push(("position", Value::Int(at)));
            told = Some((at, Instant::now()));
        }
    } else {
        values.push(("position", Value::Null));
        told = None;
    }
    Reading {
        values,
        at: Instant::now(),
        playing,
        track: polled.track,
        told,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home, as `GetZoneGroupState` answers it: a stereo pair grouped with
    /// a kitchen speaker, a lone office speaker, a soundbar with a sub.
    fn answer() -> String {
        let state = r#"<ZoneGroups>
<ZoneGroup Coordinator="RINCON_AAA01400" ID="RINCON_AAA01400:1">
<ZoneGroupMember UUID="RINCON_AAA01400" Location="http://192.168.1.11:1400/xml/device_description.xml" ZoneName="Living" ChannelMapSet="RINCON_AAA01400:LF,LF;RINCON_BBB01400:RF,RF"/>
<ZoneGroupMember UUID="RINCON_BBB01400" Location="http://192.168.1.12:1400/xml/device_description.xml" ZoneName="Living" Invisible="1" ChannelMapSet="RINCON_AAA01400:LF,LF;RINCON_BBB01400:RF,RF"/>
<ZoneGroupMember UUID="RINCON_CCC01400" Location="http://192.168.1.13:1400/xml/device_description.xml" ZoneName="Kitchen &amp; Co"/>
</ZoneGroup>
<ZoneGroup Coordinator="RINCON_DDD01400" ID="RINCON_DDD01400:2">
<ZoneGroupMember UUID="RINCON_DDD01400" Location="http://192.168.1.14:1400/xml/device_description.xml" ZoneName="Office"/>
</ZoneGroup>
<ZoneGroup Coordinator="RINCON_EEE01400" ID="RINCON_EEE01400:3">
<ZoneGroupMember UUID="RINCON_EEE01400" Location="http://192.168.1.15:1400/xml/device_description.xml" ZoneName="Den" HTSatChanMapSet="RINCON_EEE01400:LF,RF;RINCON_FFF01400:SW">
<Satellite UUID="RINCON_FFF01400" Location="http://192.168.1.16:1400/xml/device_description.xml" ZoneName="Den" Invisible="1"/>
</ZoneGroupMember>
</ZoneGroup>
</ZoneGroups>"#;
        format!(
            "<s:Envelope><s:Body><u:GetZoneGroupStateResponse><ZoneGroupState>{}</ZoneGroupState></u:GetZoneGroupStateResponse></s:Body></s:Envelope>",
            moli_net::upnp::escape(state)
        )
    }

    #[test]
    fn reads_the_groups_of_the_home() {
        let groups = parse_topology(&answer());
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].coordinator, "RINCON_AAA01400");
        assert_eq!(groups[0].members.len(), 3);
        assert_eq!(groups[2].members.len(), 1, "a satellite is not a member");
        assert!(parse_topology("<nothing/>").is_empty());
    }

    #[test]
    fn a_stereo_pair_is_one_room() {
        let (rooms, hidden) = rooms(&parse_topology(&answer()));
        let names: Vec<&str> = rooms.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Living", "Kitchen & Co", "Office", "Den"]);
        assert!(hidden.contains("RINCON_BBB01400"));
        assert!(!hidden.contains("RINCON_AAA01400"));
        assert!(rooms[0].pair);
        assert!(!rooms[1].pair);
    }

    #[test]
    fn a_group_follows_its_leader() {
        let (rooms, _) = rooms(&parse_topology(&answer()));
        let (living, kitchen, office) = (&rooms[0], &rooms[1], &rooms[2]);
        assert_eq!(living.with, ["Kitchen & Co"]);
        assert_eq!(kitchen.with, ["Living"]);
        assert_eq!(kitchen.leader, "Living");
        assert_eq!(kitchen.leader_host, "192.168.1.11");
        assert!(office.with.is_empty());
        assert_eq!(office.leader, "Office");
        // Playback goes to the leader, volume stays with the room.
        assert_eq!(target(kitchen, "playing"), "192.168.1.11");
        assert_eq!(target(kitchen, "volume"), "192.168.1.13");
        assert_eq!(target(kitchen, "mute"), "192.168.1.13");
    }

    #[test]
    fn reads_attributes_by_name() {
        let head = r#"UUID="A" ZoneName="Caf&amp;é" Invisible="1""#;
        assert_eq!(attr(head, "ZoneName").as_deref(), Some("Caf&é"));
        assert_eq!(attr(head, "Invisible").as_deref(), Some("1"));
        assert_eq!(attr(head, "Name"), None, "not the end of another name");
    }

    fn read(track: &str, playing: bool, told: Option<(i64, Instant)>) -> Reading {
        Reading {
            values: Vec::new(),
            at: Instant::now(),
            playing,
            track: track.into(),
            told,
        }
    }

    #[test]
    fn a_position_is_told_when_it_moves_not_while_it_follows() {
        let ago = Instant::now()
            .checked_sub(Duration::from_secs(20))
            .expect("a clock that has run for 20 seconds");
        let prev = read("a", true, Some((10, ago)));
        // Playing for 20 s since 10 s: 30 s is where the room's clock is.
        assert!(!position_moved(Some(&prev), 31, true, "a"));
        assert!(position_moved(Some(&prev), 90, true, "a"), "a seek");
        assert!(position_moved(Some(&prev), 31, true, "b"), "another track");
        assert!(position_moved(Some(&prev), 31, false, "a"), "paused");
        assert!(position_moved(None, 0, true, "a"));
        // Paused, the clock does not run.
        let paused = read("a", false, Some((10, ago)));
        assert!(!position_moved(Some(&paused), 11, false, "a"));
        assert!(position_moved(Some(&paused), 30, false, "a"));
    }

    #[test]
    fn a_follower_is_read_with_its_leader() {
        let (rooms, _) = rooms(&parse_topology(&answer()));
        let mut home = Home::default();
        for room in rooms {
            let id = DeviceId::new(&moli_core::InstanceId::from("sonos"), &room.uuid);
            home.shown.insert(
                id,
                Shown {
                    room,
                    model: "x".into(),
                    leader: Arc::default(),
                },
            );
        }
        let who: HashSet<String> = ["RINCON_CCC01400".to_owned()].into();
        let wanted = with_group(&home, &who);
        assert_eq!(
            wanted.len(),
            2,
            "the kitchen and its leader, the living room"
        );
        assert!(wanted.contains("RINCON_AAA01400"));
        assert!(!wanted.contains("RINCON_DDD01400"));
    }
}
