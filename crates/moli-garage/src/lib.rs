//! Garage doors: a relay that presses the motor's button (a dry contact on
//! its push-button terminals), seen and commanded as a door — open, closed,
//! opening, closing, stopped — on the dashboard, by Moli and by HomeKit.
//!
//! Most motors know one button: a press opens a closed door, closes an
//! open one, stops a moving one, and the next press goes the other way.
//! Without a sensor on the door, Moli follows those presses (its own, and
//! the wall button's, seen as the relay closing without having been asked)
//! and the travel time: the position is then a belief, said as such
//! (`assumed`). With a sensor (`contact`, true when closed), the door's
//! own word wins.
//!
//! Moli runs it as instance [`INSTANCE`] whenever `moli.toml` declares doors:
//!
//! ```toml
//! [[garage]]
//! id = "garage"                       # the device is portes:garage
//! name = "Porte de garage"
//! room = "Garage"
//! relay = "zwave:…/switch"            # the relay's on/off point
//! # contact = "z2m:…/contact"         # a sensor on the door (true = closed)
//! travel = 20                          # seconds from closed to open
//! ```

use std::sync::Arc;
use std::time::Duration;

use moli_core::{
    Access, Device, DeviceId, Event, InstanceId, Kind, Origin, PointId, PointSpec, Semantic, Unit,
    Value,
};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx, Hub};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// The instance (and the devices' prefix: `portes:garage`).
pub const INSTANCE: &str = "portes";
/// Between two presses of one order (stop, then the other way).
const BETWEEN: Duration = Duration::from_millis(1500);
/// The relay closing this long after Moli's own press is that press.
const ECHO: Duration = Duration::from_secs(4);
/// A relay that does not open by itself is opened after this.
const RELEASE: Duration = Duration::from_millis(1500);

/// One door, as `moli.toml` declares it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Door {
    /// Stable id: the device is `portes:<id>`. Never rename it.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub room: Option<String>,
    /// The relay on the motor's button: its on/off point (`device/key`).
    pub relay: String,
    /// A sensor on the door (`device/key`, true when closed).
    #[serde(default)]
    pub contact: Option<String>,
    /// Seconds the door takes from one end to the other (default 20).
    #[serde(default = "default_travel")]
    pub travel: u64,
}

fn default_travel() -> u64 {
    20
}

/// The doors as declared: ids, points, travel time.
pub fn check(doors: &[Door]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for d in doors {
        if d.id.is_empty() || d.id.contains([':', '/']) {
            return Err(format!("id {:?}: non-empty, without ':' nor '/'", d.id));
        }
        if !ids.insert(&d.id) {
            return Err(format!("id {:?} twice", d.id));
        }
        for point in std::iter::once(&d.relay).chain(&d.contact) {
            if PointId::from(point.as_str()).split().is_none() {
                return Err(format!("{}: {point:?} is not a point (device/key)", d.id));
            }
        }
        if !(3..=180).contains(&d.travel) {
            return Err(format!("{}: travel from 3 to 180 seconds", d.id));
        }
    }
    Ok(())
}

impl Door {
    fn relay_point(&self) -> PointId {
        PointId::from(self.relay.as_str())
    }

    fn contact_point(&self) -> Option<PointId> {
        self.contact.as_deref().map(PointId::from)
    }

    fn relay_device(&self) -> Option<DeviceId> {
        self.relay_point().split().map(|(d, _)| d)
    }
}

/// Where the door is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Closed,
    Opening,
    Open,
    Closing,
    Stopped,
}

impl Phase {
    pub const ALL: [Phase; 5] = [
        Phase::Closed,
        Phase::Opening,
        Phase::Open,
        Phase::Closing,
        Phase::Stopped,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Opening => "opening",
            Self::Open => "open",
            Self::Closing => "closing",
            Self::Stopped => "stopped",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.word() == word)
    }

    /// Where it is headed (or was, stopped): `true` up.
    fn up(self, last_up: bool) -> bool {
        match self {
            Self::Opening | Self::Open => true,
            Self::Closing | Self::Closed => false,
            Self::Stopped => last_up,
        }
    }
}

/// What a door believes of itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Belief {
    pub phase: Phase,
    /// The last way it moved (what a press after a stop reverses).
    pub last_up: bool,
}

impl Belief {
    /// One press of the button.
    #[must_use]
    pub fn press(self) -> Self {
        let phase = match (self.phase, self.last_up) {
            (Phase::Open, _) | (Phase::Stopped, true) => Phase::Closing,
            (Phase::Opening | Phase::Closing, _) => Phase::Stopped,
            (Phase::Closed | Phase::Stopped, _) => Phase::Opening,
        };
        Self {
            phase,
            last_up: phase.up(self.last_up),
        }
    }

    /// Presses that send it towards `up` (none if it is there or going).
    pub fn presses_to(self, up: bool) -> usize {
        let mut b = self;
        for n in 0..4 {
            let there = match b.phase {
                Phase::Open | Phase::Opening => up,
                Phase::Closed | Phase::Closing => !up,
                Phase::Stopped => false,
            };
            if there {
                return n;
            }
            b = b.press();
        }
        0
    }

    /// The end of a journey.
    #[must_use]
    pub fn arrived(self) -> Self {
        let phase = match self.phase {
            Phase::Opening => Phase::Open,
            Phase::Closing => Phase::Closed,
            other => other,
        };
        Self { phase, ..self }
    }

    /// What HomeKit's target says: where it goes, or went.
    pub fn target(self) -> &'static str {
        if self.phase.up(self.last_up) {
            "open"
        } else {
            "closed"
        }
    }
}

/// The door's sensor spoke: `closed` true when shut.
fn sensed(belief: Belief, closed: bool) -> Belief {
    match (closed, belief.phase) {
        (true, _) => Belief {
            phase: Phase::Closed,
            last_up: false,
        },
        // Open while Moli thought it shut: someone opened it.
        (false, Phase::Closed) => Belief {
            phase: Phase::Opening,
            last_up: true,
        },
        (false, _) => belief,
    }
}

fn device(id: DeviceId, instance: &InstanceId, door: &Door) -> Device {
    let point = |key: &str, label: String, kind: Kind, write: bool, semantic: Semantic| PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access { read: true, write },
        unit: None,
        semantic,
    };
    let words = |list: &[&str]| Kind::Enum {
        values: list.iter().map(|w| Arc::from(*w)).collect(),
    };
    Device {
        id,
        instance: instance.clone(),
        native_name: door.name.as_str().into(),
        manufacturer: Some("Moli".into()),
        model: Some(moli_i18n::tr!("pilotes.garage.modele").into()),
        description: None,
        native_room: door.room.as_deref().map(Into::into),
        members: door.relay_device().into_iter().collect(),
        points: vec![
            point(
                "state",
                moli_i18n::tr!("pilotes.garage.etat"),
                words(&Phase::ALL.map(Phase::word)),
                false,
                Semantic::GarageDoor,
            ),
            point(
                "target",
                moli_i18n::tr!("pilotes.garage.commande"),
                words(&["open", "closed"]),
                true,
                Semantic::GarageDoor,
            ),
            point(
                "press",
                moli_i18n::tr!("pilotes.garage.impulsion"),
                Kind::Binary,
                true,
                Semantic::Other,
            ),
            point(
                "assumed",
                moli_i18n::tr!("pilotes.garage.supposee"),
                Kind::Binary,
                false,
                Semantic::Other,
            ),
            // How long a journey takes: what the dashboard animates.
            PointSpec {
                unit: Some(Unit::Second),
                ..point(
                    "travel",
                    moli_i18n::tr!("pilotes.garage.course"),
                    Kind::Numeric {
                        min: Some(3.0),
                        max: Some(180.0),
                        step: Some(1.0),
                    },
                    false,
                    Semantic::Config,
                )
            },
        ],
    }
}

#[derive(Debug)]
pub struct Garage {
    hub: Hub,
    doors: Vec<Door>,
}

impl Garage {
    #[must_use]
    pub fn new(hub: Hub, doors: Vec<Door>) -> Self {
        Self { hub, doors }
    }
}

/// One door at run time.
struct Live {
    id: DeviceId,
    belief: Belief,
    /// When the current journey ends (no sensor to say so).
    arrives: Option<Instant>,
    /// Moli's own last press (the relay's echo is not the wall button).
    pressed: Option<Instant>,
}

enum Next {
    Command(CommandRequest),
    Event(Result<Event, RecvError>),
    Arrived,
    Stop,
}

fn publish(ctx: &DriverCtx, door: &Door, live: &Live) {
    ctx.set_state(&live.id, "state", Value::from(live.belief.phase.word()));
    ctx.set_state(&live.id, "target", Value::from(live.belief.target()));
    ctx.set_state(&live.id, "assumed", Value::Bool(door.contact.is_none()));
    ctx.set_state(
        &live.id,
        "travel",
        Value::Int(i64::try_from(door.travel).unwrap_or(20)),
    );
}

/// The door moves (or stops): its journey ends after its travel time.
fn moved(ctx: &DriverCtx, door: &Door, live: &mut Live, belief: Belief) {
    live.belief = belief;
    live.arrives = matches!(belief.phase, Phase::Opening | Phase::Closing)
        .then(|| Instant::now() + Duration::from_secs(door.travel));
    publish(ctx, door, live);
}

impl Garage {
    /// Presses the button `n` times, a moment apart; the belief follows
    /// each press that went through.
    async fn press(
        &self,
        ctx: &DriverCtx,
        door: &Door,
        live: &mut Live,
        n: usize,
    ) -> Result<(), String> {
        let actor = moli_i18n::tr!("pilotes.garage.acteur", name = door.name);
        for i in 0..n {
            if i > 0 {
                tokio::time::sleep(BETWEEN).await;
            }
            live.pressed = Some(Instant::now());
            self.hub
                .command(
                    &door.relay_point(),
                    Value::Bool(true),
                    Origin::System,
                    Some(actor.clone()),
                )
                .await
                .map_err(|e| e.to_string())?;
            moved(ctx, door, live, live.belief.press());
            // A relay that stays closed is released (most open by themselves).
            let hub = self.hub.clone();
            let point = door.relay_point();
            let actor = actor.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RELEASE).await;
                if hub
                    .state(&point)
                    .is_some_and(|s| s.value == Value::Bool(true))
                {
                    let _ = hub
                        .command(&point, Value::Bool(false), Origin::System, Some(actor))
                        .await;
                }
            });
        }
        Ok(())
    }

    async fn command(&self, ctx: &DriverCtx, lives: &mut [Live], command: CommandRequest) {
        if command.abandoned() {
            return;
        }
        let Some(i) = lives.iter().position(|l| l.id == command.device.id) else {
            command.reply(Err(moli_i18n::tr!("pilotes.garage.inconnue")));
            return;
        };
        let door = &self.doors[i];
        let presses = match (&*command.key, &command.value) {
            ("press", _) => 1,
            ("target", Value::Text(t)) if &**t == "open" => lives[i].belief.presses_to(true),
            ("target", Value::Text(t)) if &**t == "closed" => lives[i].belief.presses_to(false),
            _ => {
                command.reply(Err(moli_i18n::tr!("pilotes.garage.ordre_inconnu")));
                return;
            }
        };
        let result = self.press(ctx, door, &mut lives[i], presses).await;
        command.reply(result);
    }

    /// The relay or the sensor of a door changed.
    fn on_state(&self, ctx: &DriverCtx, lives: &mut [Live], point: &PointId, value: &Value) {
        for (door, live) in self.doors.iter().zip(lives.iter_mut()) {
            if door.contact_point().as_ref() == Some(point) {
                if let Value::Bool(closed) = value {
                    let belief = sensed(live.belief, *closed);
                    if belief != live.belief {
                        moved(ctx, door, live, belief);
                        if belief.phase == Phase::Closed {
                            live.arrives = None;
                        }
                    }
                }
            } else if door.relay_point() == *point && *value == Value::Bool(true) {
                let ours = live.pressed.is_some_and(|t| t.elapsed() < ECHO);
                if !ours {
                    // The wall button: one press the motor saw too.
                    moved(ctx, door, live, live.belief.press());
                }
            }
        }
    }
}

async fn until(when: Option<Instant>) {
    match when {
        Some(when) => tokio::time::sleep_until(when).await,
        None => std::future::pending().await,
    }
}

impl Driver for Garage {
    fn kind(&self) -> &'static str {
        "garage"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let mut lives: Vec<Live> = Vec::new();
            for door in &self.doors {
                let id = ctx.device_id(&door.id);
                ctx.upsert_device(device(id.clone(), ctx.instance(), door));
                // Where it was (the state cache); a journey under way at the
                // stop is over by now.
                let phase = ctx
                    .current(&id, "state")
                    .and_then(|v| match v {
                        Value::Text(t) => Phase::from_word(&t),
                        _ => None,
                    })
                    .unwrap_or(Phase::Closed);
                let mut live = Live {
                    id,
                    belief: Belief {
                        phase,
                        last_up: phase.up(false),
                    }
                    .arrived(),
                    arrives: None,
                    pressed: None,
                };
                if let Some(Value::Bool(closed)) = door
                    .contact_point()
                    .and_then(|p| self.hub.state(&p))
                    .map(|s| s.value)
                {
                    live.belief = sensed(live.belief, closed).arrived();
                }
                publish(ctx, door, &live);
                let online = door
                    .relay_device()
                    .and_then(|d| self.hub.device(&d))
                    .is_some_and(|v| v.online != Some(false));
                ctx.set_availability(&live.id, online);
                lives.push(live);
            }
            ctx.ready();
            let mut events = self.hub.subscribe();
            loop {
                let soonest = lives.iter().filter_map(|l| l.arrives).min();
                let next = tokio::select! {
                    command = ctx.next_command() => command.map_or(Next::Stop, Next::Command),
                    event = events.recv() => Next::Event(event),
                    () = until(soonest) => Next::Arrived,
                };
                match next {
                    Next::Stop | Next::Event(Err(RecvError::Closed)) => return Ok(()),
                    Next::Command(command) => self.command(ctx, &mut lives, command).await,
                    Next::Event(Ok(Event::State { point, value, .. })) => {
                        self.on_state(ctx, &mut lives, &point, &value);
                    }
                    Next::Event(Ok(Event::Availability { device, online })) => {
                        for (door, live) in self.doors.iter().zip(&lives) {
                            if door.relay_device().as_ref() == Some(&device) {
                                ctx.set_availability(&live.id, online);
                            }
                        }
                    }
                    Next::Event(Ok(_) | Err(RecvError::Lagged(_))) => {}
                    Next::Arrived => {
                        let now = Instant::now();
                        for (door, live) in self.doors.iter().zip(lives.iter_mut()) {
                            if live.arrives.is_some_and(|t| t <= now) {
                                live.arrives = None;
                                live.belief = live.belief.arrived();
                                publish(ctx, door, live);
                            }
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(phase: Phase, last_up: bool) -> Belief {
        Belief { phase, last_up }
    }

    #[test]
    fn one_button_four_moves() {
        let mut b = at(Phase::Closed, false);
        let seen: Vec<Phase> = (0..5)
            .map(|_| {
                b = b.press();
                b.phase
            })
            .collect();
        assert_eq!(
            seen,
            [
                Phase::Opening,
                Phase::Stopped,
                Phase::Closing,
                Phase::Stopped,
                Phase::Opening
            ]
        );
        assert_eq!(at(Phase::Open, true).press().phase, Phase::Closing);
        assert_eq!(at(Phase::Opening, true).arrived().phase, Phase::Open);
        assert_eq!(at(Phase::Closing, false).arrived().phase, Phase::Closed);
    }

    #[test]
    fn as_many_presses_as_it_takes() {
        assert_eq!(at(Phase::Closed, false).presses_to(true), 1);
        assert_eq!(at(Phase::Closed, false).presses_to(false), 0);
        assert_eq!(at(Phase::Opening, true).presses_to(true), 0);
        // Closing, wanted open: stop, then up.
        assert_eq!(at(Phase::Closing, false).presses_to(true), 2);
        // Stopped on the way up: the next press goes down.
        assert_eq!(at(Phase::Stopped, true).presses_to(false), 1);
        // Stopped on the way up, wanted up: down, stop, up.
        assert_eq!(at(Phase::Stopped, true).presses_to(true), 3);
        assert_eq!(at(Phase::Open, true).target(), "open");
        assert_eq!(at(Phase::Stopped, false).target(), "closed");
    }

    #[test]
    fn the_sensor_wins() {
        assert_eq!(sensed(at(Phase::Opening, true), true).phase, Phase::Closed);
        assert_eq!(
            sensed(at(Phase::Closed, false), false).phase,
            Phase::Opening
        );
        assert_eq!(sensed(at(Phase::Open, true), false).phase, Phase::Open);
    }

    #[test]
    fn declared_doors_are_checked() {
        let door = |id: &str, relay: &str, travel| Door {
            id: id.into(),
            name: "Porte".into(),
            room: None,
            relay: relay.into(),
            contact: None,
            travel,
        };
        assert!(check(&[door("garage", "zwave:a-37/switch", 20)]).is_ok());
        assert!(check(&[door("a/b", "zwave:a-37/switch", 20)]).is_err());
        assert!(check(&[door("garage", "zwave:a-37", 20)]).is_err());
        assert!(check(&[door("garage", "zwave:a-37/switch", 1)]).is_err());
        assert!(
            check(&[
                door("garage", "zwave:a-37/switch", 20),
                door("garage", "zwave:a-38/switch", 20)
            ])
            .is_err()
        );
    }
}
