//! Groups of shutters: several shutters, chosen from the dashboard, seen and
//! commanded as **one** (« Pièce de vie »). A shutter stays itself, and may
//! be in any number of groups.
//!
//! A group is a device of instance [`INSTANCE`] with a shutter's own points
//! (`control`, `percent_control`, `slats_open`): the dashboard's card, the
//! automations, HomeKit and Alexa take it for a shutter, because that is what
//! it looks like. Its `aid` follows from its device id, which never changes.
//!
//! - **Position shown**: the mean of the reachable members whose position is
//!   known, rounded. One at 100 and one at 0 read 50.
//! - **A member that does not answer**: its last position no longer counts,
//!   and it is not sent the order (none would be kept for later). The others
//!   obey; `unreachable` says how many are missing. Nobody reachable: the
//!   group is offline.
//! - **The answer to an order**: fine as soon as one member obeyed, otherwise
//!   the first refusal (a position asked of shutters that do not know theirs).
//!   Each member's own order is in the journal, with its result.
//! - **The guard** judges the group on its members' rooms (`Device::members`).
//!   The orders it then gives are the system's: already judged.
//! - **No group inside a group**, and a group is never someone's member.

mod store;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::future::join_all;
use moli_core::{
    Access, Device, DeviceId, Event, InstanceId, Kind, Origin, PointId, PointSpec, Semantic, Unit,
    Value,
};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx, Hub};
pub use store::{Group, GroupError, Groups, MAX_GROUPS, MAX_MEMBERS, MAX_NAME};
use tokio::sync::broadcast::error::RecvError;

/// The instance (and the devices' prefix: `groupes-stores:g1`).
pub const INSTANCE: &str = "groupes-stores";
/// What the dashboard recognizes a group by: never translated.
pub const MODEL: &str = "CoverGroup";

const CONTROL: &str = "control";
const PERCENT: &str = "percent_control";
const STATE: &str = "percent_state";
const SLATS: &str = "slats_open";
const UNREACHABLE: &str = "unreachable";

// ---- what a member is -------------------------------------------------------------------

/// A shutter's points, as the dashboard, HomeKit and Alexa read them: a
/// `percent_control`, or a writable `control` offering `open`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Keys {
    /// Sets the position, with the range it takes.
    set: Option<(f64, f64)>,
    /// Tells the position, with its range.
    position: Option<(Arc<str>, f64, f64)>,
    opens: bool,
    closes: bool,
    stops: bool,
}

fn range(p: &PointSpec) -> Option<(f64, f64)> {
    match &p.kind {
        Kind::Numeric { min, max, .. } => {
            let (lo, hi) = (min.unwrap_or(0.0), max.unwrap_or(100.0));
            (hi > lo).then_some((lo, hi))
        }
        _ => None,
    }
}

fn keys(device: &Device) -> Option<Keys> {
    let percent = device.point(PERCENT);
    let set = percent.filter(|p| p.access.write).and_then(range);
    let position = device
        .point(STATE)
        .filter(|p| range(p).is_some())
        .or(percent)
        .filter(|p| p.access.read)
        .and_then(|p| range(p).map(|(lo, hi)| (p.key.clone(), lo, hi)));
    let offers = |word: &str| {
        device.point(CONTROL).is_some_and(|p| {
            p.access.write
                && matches!(&p.kind, Kind::Enum { values } if values.iter().any(|v| &**v == word))
        })
    };
    let keys = Keys {
        set,
        position,
        opens: offers("open"),
        closes: offers("close"),
        stops: offers("stop"),
    };
    (keys.set.is_some() || keys.opens).then_some(keys)
}

/// Whether this device is a shutter (what may enter a group).
#[must_use]
pub fn is_cover(device: &Device) -> bool {
    keys(device).is_some()
}

/// An order to a group, and so to each member.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Order {
    Open,
    Stop,
    Close,
    /// Percent.
    To(f64),
}

fn parse(key: &str, value: &Value) -> Option<Order> {
    match (key, value) {
        (CONTROL, Value::Text(t)) => match &**t {
            "open" => Some(Order::Open),
            "stop" => Some(Order::Stop),
            "close" => Some(Order::Close),
            _ => None,
        },
        (PERCENT, v) => v.as_f64().map(|pct| Order::To(pct.clamp(0.0, 100.0))),
        _ => None,
    }
}

/// What this order is to this member: `None` when it has nothing to do with
/// it (a stop to a shutter that cannot stop). A member that only opens and
/// closes goes to the nearest end.
fn order_for(keys: &Keys, order: Order) -> Option<(&'static str, Value)> {
    let span = |pct: f64| {
        keys.set.map(|(lo, hi)| {
            (
                PERCENT,
                Value::Float((lo + (hi - lo) * pct / 100.0).round()),
            )
        })
    };
    let word = |w: &'static str| (CONTROL, Value::from(w));
    match order {
        Order::Stop => keys.stops.then(|| word("stop")),
        Order::Open => keys.opens.then(|| word("open")).or_else(|| span(100.0)),
        Order::Close => keys.closes.then(|| word("close")).or_else(|| span(0.0)),
        Order::To(pct) => span(pct.round()).or_else(|| {
            if pct >= 50.0 {
                keys.opens.then(|| word("open"))
            } else {
                keys.closes.then(|| word("close"))
            }
        }),
    }
}

// ---- what the group shows ---------------------------------------------------------------

/// A member, now.
#[derive(Clone, Debug, Default, PartialEq)]
struct Member {
    answers: bool,
    /// Percent.
    position: Option<f64>,
    slats_open: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Shown {
    /// Someone can obey.
    online: bool,
    /// `None`: nobody reachable knows where it is.
    position: Option<i64>,
    /// Light comes through somewhere.
    slats_open: bool,
    unreachable: i64,
}

fn shown(members: &[Member]) -> Shown {
    let answering: Vec<&Member> = members.iter().filter(|m| m.answers).collect();
    let known: Vec<f64> = answering.iter().filter_map(|m| m.position).collect();
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let position = (!known.is_empty())
        .then(|| (known.iter().sum::<f64>() / known.len() as f64).round() as i64);
    #[allow(clippy::cast_possible_wrap)]
    let unreachable = (members.len() - answering.len()) as i64;
    Shown {
        online: !answering.is_empty(),
        position,
        slats_open: answering.iter().any(|m| m.slats_open),
        unreachable,
    }
}

/// The members as the hub knows them now.
fn read(hub: &Hub, group: &Group) -> Vec<(Member, Option<Keys>)> {
    group
        .members
        .iter()
        .map(|id| {
            let Some(view) = hub.device(&DeviceId::from(id.as_str())) else {
                return (Member::default(), None);
            };
            let k = keys(&view.device);
            let number = |key: &str| view.state.get(key).and_then(|s| s.value.as_f64());
            let position =
                k.as_ref()
                    .and_then(|k| k.position.as_ref())
                    .and_then(|(key, lo, hi)| {
                        Some(((number(key)? - lo) * 100.0 / (hi - lo)).clamp(0.0, 100.0))
                    });
            let member = Member {
                answers: k.is_some() && view.online != Some(false),
                position,
                slats_open: view
                    .state
                    .get(SLATS)
                    .is_some_and(|s| s.value == Value::Bool(true)),
            };
            (member, k)
        })
        .collect()
}

// ---- the device -------------------------------------------------------------------------

fn spec(key: &str, label: &str, kind: Kind, access: Access, unit: Option<Unit>) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access,
        unit,
        semantic: Semantic::Other,
    }
}

fn device(id: DeviceId, instance: &InstanceId, group: &Group) -> Device {
    let read_only = Access {
        read: true,
        write: false,
    };
    Device {
        id,
        instance: instance.clone(),
        native_name: group.name.as_str().into(),
        manufacturer: Some("Moli".into()),
        model: Some(MODEL.into()),
        description: None,
        native_room: None,
        members: group
            .members
            .iter()
            .map(|m| DeviceId::from(m.as_str()))
            .collect(),
        points: vec![
            spec(
                CONTROL,
                &moli_i18n::tr!("pilotes.covers.commande"),
                Kind::Enum {
                    values: vec!["open".into(), "stop".into(), "close".into()],
                },
                Access {
                    read: false,
                    write: true,
                },
                None,
            ),
            spec(
                PERCENT,
                &moli_i18n::tr!("pilotes.covers.ouverture"),
                Kind::Numeric {
                    min: Some(0.0),
                    max: Some(100.0),
                    step: Some(1.0),
                },
                Access {
                    read: true,
                    write: true,
                },
                Some(Unit::Percent),
            ),
            spec(
                SLATS,
                &moli_i18n::tr!("pilotes.covers.ajoure"),
                Kind::Binary,
                read_only,
                None,
            ),
            spec(
                UNREACHABLE,
                &moli_i18n::tr!("pilotes.covers.injoignables"),
                Kind::Numeric {
                    min: Some(0.0),
                    max: None,
                    step: Some(1.0),
                },
                read_only,
                None,
            ),
        ],
    }
}

/// Gives the members the order, side by side: they leave together. A member
/// that does not answer is left out.
async fn give(hub: &Hub, group: &Group, order: Order) -> Result<(), String> {
    let actor = moli_i18n::tr!("pilotes.covers.acteur", name = group.name);
    let reading = read(hub, group);
    if !reading.iter().any(|(m, _)| m.answers) {
        return Err(moli_i18n::tr!("pilotes.covers.personne"));
    }
    let runs = group
        .members
        .iter()
        .zip(&reading)
        .filter(|(_, (member, _))| member.answers)
        .filter_map(|(id, (_, k))| {
            let (key, value) = order_for(k.as_ref()?, order)?;
            let point = PointId::new(&DeviceId::from(id.as_str()), key);
            let actor = actor.clone();
            Some(async move {
                hub.command(&point, value, Origin::System, Some(actor))
                    .await
                    .map_err(|e| (point, e.to_string()))
            })
        });
    let results = join_all(runs).await;
    let obeyed = results.is_empty() || results.iter().any(Result::is_ok);
    let mut first = None;
    for (point, error) in results.into_iter().filter_map(Result::err) {
        tracing::warn!(group = %group.id, %point, %error, "cover group: a member did not obey");
        first.get_or_insert(error);
    }
    match first {
        Some(error) if !obeyed => Err(error),
        _ => Ok(()),
    }
}

// ---- the driver -------------------------------------------------------------------------

#[derive(Debug)]
pub struct Covers {
    hub: Hub,
    groups: Groups,
}

impl Covers {
    #[must_use]
    pub fn new(hub: Hub, groups: Groups) -> Self {
        Self { hub, groups }
    }

    /// Publishes what changed for one group.
    fn refresh(&self, ctx: &DriverCtx, group: &Group, published: &mut Published) {
        let id = group.device_id();
        let device = device(id.clone(), ctx.instance(), group);
        if published.device.as_ref() != Some(&device) {
            ctx.upsert_device(device.clone());
            published.device = Some(device);
            published.shown = None;
        }
        let members: Vec<Member> = read(&self.hub, group).into_iter().map(|(m, _)| m).collect();
        let now = shown(&members);
        let before = published.shown.take();
        match now.position {
            Some(position) => {
                if before.as_ref().is_none_or(|b| b.position != now.position) {
                    ctx.set_state(&id, PERCENT, Value::Int(position));
                }
            }
            // Members reachable and none knows: unknown, not what it last
            // was. Nobody reachable (a restart, say): the last one stands.
            None if now.online && before.as_ref().is_none_or(|b| b.position.is_some()) => {
                ctx.set_state(&id, PERCENT, Value::Null);
            }
            None => {}
        }
        if before
            .as_ref()
            .is_none_or(|b| b.slats_open != now.slats_open)
        {
            ctx.set_state(&id, SLATS, Value::Bool(now.slats_open));
        }
        if before
            .as_ref()
            .is_none_or(|b| b.unreachable != now.unreachable)
        {
            ctx.set_state(&id, UNREACHABLE, Value::Int(now.unreachable));
        }
        if before.as_ref().is_none_or(|b| b.online != now.online) {
            ctx.set_availability(&id, now.online);
        }
        published.shown = Some(now);
    }

    /// The groups as they are now: the devices of those gone removed (of
    /// those deleted while Moli was stopped too), the others published.
    fn sync(&self, ctx: &DriverCtx, house: &mut House) {
        let groups = self.groups.list();
        let ids: Vec<DeviceId> = groups.iter().map(Group::device_id).collect();
        let mut gone = ctx.devices();
        gone.extend(ctx.known_devices());
        for id in gone.iter().filter(|id| !ids.contains(id)) {
            ctx.remove_device(id);
        }
        house
            .published
            .retain(|id, _| groups.iter().any(|g| &g.id == id));
        house.members.clear();
        for group in &groups {
            for member in &group.members {
                house
                    .members
                    .entry(DeviceId::from(member.as_str()))
                    .or_default()
                    .push(group.id.clone());
            }
            self.refresh(
                ctx,
                group,
                house.published.entry(group.id.clone()).or_default(),
            );
        }
        house.groups = groups.into_iter().map(|g| (g.id.clone(), g)).collect();
    }

    /// Answered from its own task: a member slow to answer holds back neither
    /// the next order (a stop) nor what the group shows.
    fn command(&self, house: &House, command: CommandRequest) {
        if command.abandoned() {
            return;
        }
        let native = command.device.id.as_str().split_once(':').map(|(_, n)| n);
        let Some(group) = native.and_then(|n| house.groups.get(n)).cloned() else {
            command.reply(Err(moli_i18n::tr!("pilotes.covers.groupe_inconnu")));
            return;
        };
        let Some(order) = parse(&command.key, &command.value) else {
            let why = moli_i18n::tr!("pilotes.covers.commande_inconnue", key = command.key);
            command.reply(Err(why));
            return;
        };
        let hub = self.hub.clone();
        tokio::spawn(async move {
            let result = give(&hub, &group, order).await;
            command.reply(result);
        });
    }
}

/// What the driver last published for a group.
#[derive(Default)]
struct Published {
    device: Option<Device>,
    shown: Option<Shown>,
}

/// The driver's view of the groups.
#[derive(Default)]
struct House {
    groups: BTreeMap<String, Group>,
    /// Which groups each shutter is in.
    members: HashMap<DeviceId, Vec<String>>,
    published: HashMap<String, Published>,
}

enum Next {
    Command(CommandRequest),
    Event(Result<Event, RecvError>),
    Changed,
    Stop,
}

impl Driver for Covers {
    fn kind(&self) -> &'static str {
        "cover_groups"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            // Subscribe before reading: nothing falls in between.
            let mut events = self.hub.subscribe();
            let mut edits = self.groups.watch();
            let mut house = House::default();
            edits.mark_unchanged();
            self.sync(ctx, &mut house);
            ctx.ready();
            loop {
                // No group: the bus is left alone, nothing wakes this task.
                let idle = house.groups.is_empty();
                let next = tokio::select! {
                    command = ctx.next_command() => command.map_or(Next::Stop, Next::Command),
                    edited = edits.changed() => match edited {
                        Ok(()) => Next::Changed,
                        Err(_) => Next::Stop,
                    },
                    event = events.recv(), if !idle => Next::Event(event),
                };
                let touched: Vec<String> = match next {
                    Next::Stop | Next::Event(Err(RecvError::Closed)) => return Ok(()),
                    Next::Command(command) => {
                        self.command(&house, command);
                        Vec::new()
                    }
                    Next::Changed => {
                        self.sync(ctx, &mut house);
                        Vec::new()
                    }
                    Next::Event(Err(RecvError::Lagged(_))) => {
                        house.groups.keys().cloned().collect()
                    }
                    Next::Event(Ok(event)) => {
                        let device = match &event {
                            Event::State { point, .. } => point.split().map(|(d, _)| d),
                            Event::Availability { device, .. }
                            | Event::DeviceRemoved { device } => Some(device.clone()),
                            Event::DeviceUpserted { device } => Some(device.id.clone()),
                            _ => None,
                        };
                        device
                            .and_then(|d| house.members.get(&d).cloned())
                            .unwrap_or_default()
                    }
                };
                for id in touched {
                    if let Some(group) = house.groups.get(&id) {
                        let published = house.published.entry(id).or_default();
                        self.refresh(ctx, group, published);
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
