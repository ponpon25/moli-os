//! Changes nobody ordered through Moli (a wall button, a speaker's own
//! controls) are journaled as `External`, in watched rooms only.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use moli_core::{
    Access, Action, Device, InstanceId, Kind, Origin, PointId, PointSpec, Semantic, Value,
};
use moli_runtime::{BoxFuture, Driver, DriverCtx, Hub, HubOptions, LabelPatch, spawn_driver};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

type Poke = (&'static str, Value, bool);

/// A speaker and its wall button: states change when a test says so, or when
/// an order arrives (the device echoes it, like a real one).
struct Room {
    pokes: Mutex<Option<mpsc::UnboundedReceiver<Poke>>>,
}

fn spec(key: &str, kind: Kind, write: bool) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: key.into(),
        kind,
        access: Access { read: true, write },
        unit: None,
        semantic: Semantic::infer(key, None),
    }
}

impl Driver for Room {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let mut pokes = self.pokes.lock().unwrap().take().expect("run once");
            let id = ctx.device_id("speaker");
            ctx.upsert_device(Device {
                id: id.clone(),
                instance: ctx.instance().clone(),
                native_name: "speaker".into(),
                manufacturer: None,
                model: None,
                description: None,
                native_room: None,
                members: Vec::new(),
                points: vec![
                    spec("state", Kind::Text, false),
                    spec(
                        "volume",
                        Kind::Numeric {
                            min: None,
                            max: None,
                            step: None,
                        },
                        true,
                    ),
                    spec("action", Kind::Text, false),
                ],
            });
            ctx.set_state(&id, "state", Value::Text("playing".into()));
            ctx.set_state(&id, "volume", Value::Int(5));
            ctx.ready();
            loop {
                tokio::select! {
                    cmd = ctx.next_command() => {
                        let Some(cmd) = cmd else { break };
                        ctx.set_state(&cmd.device.id, &cmd.key, cmd.value.clone());
                        cmd.reply(Ok(()));
                    }
                    poke = pokes.recv() => {
                        let Some((key, value, pulse)) = poke else { break };
                        if pulse {
                            ctx.pulse_state(&id, key, value);
                        } else {
                            ctx.set_state(&id, key, value);
                        }
                    }
                }
            }
            Ok(())
        })
    }
}

async fn start(
    opts: HubOptions,
    room: Option<&str>,
) -> (Hub, mpsc::UnboundedSender<Poke>, CancellationToken) {
    let hub = Hub::new(opts).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    spawn_driver(
        &hub,
        InstanceId::from("fake"),
        Arc::new(Room {
            pokes: Mutex::new(Some(rx)),
        }),
        cancel.clone(),
    );
    for _ in 0..200 {
        if hub.stats().drivers_running == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if let Some(room) = room {
        let patch = LabelPatch {
            name: None,
            room: Some(room.into()),
            voice: None,
        };
        hub.set_label(&"fake:speaker".into(), patch, Origin::Ui, None)
            .await
            .unwrap();
    }
    (hub, tx, cancel)
}

fn watching(room: &str) -> HubOptions {
    HubOptions {
        watch_rooms: vec![room.into()],
        ..HubOptions::default()
    }
}

fn point(key: &str) -> PointId {
    PointId::from(format!("fake:speaker/{key}"))
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}

fn observed(hub: &Hub) -> Vec<(String, Option<Value>, Value)> {
    let mut seen: Vec<_> = hub
        .journal(50)
        .iter()
        .filter(|e| e.origin == Origin::External)
        .filter_map(|e| match &e.action {
            Action::Observed { point, from, to } => {
                Some((point.as_str().to_owned(), from.clone(), to.clone()))
            }
            _ => None,
        })
        .collect();
    seen.reverse();
    seen
}

#[tokio::test]
async fn a_pause_by_hand_in_a_watched_room_is_journaled() {
    let (hub, poke, _cancel) = start(watching("Chambre Axel"), Some("chambre axel")).await;
    poke.send(("state", Value::Text("paused_playback".into()), false))
        .unwrap();
    settle().await;
    assert_eq!(
        observed(&hub),
        vec![(
            "fake:speaker/state".to_owned(),
            Some(Value::Text("playing".into())),
            Value::Text("paused_playback".into())
        )]
    );
    // The entry says nobody asked: no actor, not an order's origin.
    let entry = hub.journal(1).remove(0);
    assert_eq!(entry.origin, Origin::External);
    assert!(entry.actor.is_none());
}

#[tokio::test]
async fn a_wall_button_press_is_journaled_but_not_its_release() {
    let (hub, poke, _cancel) = start(watching("Chambre Axel"), Some("Chambre Axel")).await;
    poke.send(("action", Value::Text("input_1_single".into()), true))
        .unwrap();
    poke.send(("action", Value::Text("input_1_off".into()), true))
        .unwrap();
    settle().await;
    assert_eq!(
        observed(&hub),
        vec![(
            "fake:speaker/action".to_owned(),
            None,
            Value::Text("input_1_single".into())
        )]
    );
}

#[tokio::test]
async fn what_an_order_explains_is_not_an_outside_gesture() {
    let (hub, _poke, _cancel) = start(watching("Chambre Axel"), Some("Chambre Axel")).await;
    hub.command(&point("volume"), Value::Int(9), Origin::Automation, None)
        .await
        .unwrap();
    settle().await;
    assert!(observed(&hub).is_empty(), "{:?}", observed(&hub));
}

#[tokio::test]
async fn a_press_explains_the_movement_that_follows_it() {
    let (hub, poke, _cancel) = start(watching("Chambre Axel"), Some("Chambre Axel")).await;
    poke.send(("action", Value::Text("input_1_single".into()), true))
        .unwrap();
    poke.send(("state", Value::Text("paused_playback".into()), false))
        .unwrap();
    settle().await;
    // Only the press: what the device does right after it is not a second gesture.
    let seen = observed(&hub);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, "fake:speaker/action");
}

#[tokio::test]
async fn rooms_nobody_watches_stay_out_of_the_journal() {
    let (hub, poke, _cancel) = start(watching("Chambre Axel"), Some("Salon")).await;
    poke.send(("state", Value::Text("paused_playback".into()), false))
        .unwrap();
    poke.send(("action", Value::Text("input_1_single".into()), true))
        .unwrap();
    settle().await;
    assert!(observed(&hub).is_empty());

    // And without any watched room at all, nothing is looked at.
    let (hub, poke, _cancel) = start(HubOptions::default(), Some("Chambre Axel")).await;
    poke.send(("state", Value::Text("paused_playback".into()), false))
        .unwrap();
    settle().await;
    assert!(observed(&hub).is_empty());
}

#[tokio::test]
async fn a_track_still_starting_is_not_a_gesture() {
    let (hub, poke, _cancel) = start(watching("Chambre Axel"), Some("Chambre Axel")).await;
    poke.send(("state", Value::Text("transitioning".into()), false))
        .unwrap();
    settle().await;
    assert!(observed(&hub).is_empty());
}
