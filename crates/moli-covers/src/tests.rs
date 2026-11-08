//! The groups against a real hub and fake shutters.

use std::sync::Mutex;
use std::time::Duration;

use moli_runtime::{HubOptions, spawn_driver};
use tokio_util::sync::CancellationToken;

use super::*;

/// Shutters `a`, `b`, `c`, the opener `d` (no position), the lamp `lamp`,
/// and the wall switch `switch` (an order to its `action` is a press).
#[derive(Debug, Default)]
struct Fake {
    orders: Mutex<Vec<(String, Value)>>,
}

fn point(key: &str, kind: Kind) -> PointSpec {
    spec(
        key,
        key,
        kind,
        Access {
            read: true,
            write: true,
        },
        None,
    )
}

fn percent() -> PointSpec {
    point(
        PERCENT,
        Kind::Numeric {
            min: Some(0.0),
            max: Some(100.0),
            step: Some(1.0),
        },
    )
}

fn control() -> PointSpec {
    point(
        CONTROL,
        Kind::Enum {
            values: vec!["open".into(), "stop".into(), "close".into()],
        },
    )
}

impl Driver for Fake {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let press = point(
                PRESS,
                Kind::Enum {
                    values: vec!["on_top".into(), "off_top".into(), "on_bottom".into()],
                },
            );
            let shapes: [(&str, Vec<PointSpec>); 6] = [
                ("a", vec![percent(), control(), point("gone", Kind::Binary)]),
                ("b", vec![percent(), control(), point("gone", Kind::Binary)]),
                ("c", vec![percent(), control(), point("gone", Kind::Binary)]),
                ("d", vec![control(), point("gone", Kind::Binary)]),
                ("lamp", vec![point("on", Kind::Binary)]),
                ("switch", vec![press]),
            ];
            for (name, points) in shapes {
                ctx.upsert_device(Device {
                    id: ctx.device_id(name),
                    instance: ctx.instance().clone(),
                    native_name: name.into(),
                    manufacturer: None,
                    model: None,
                    description: None,
                    native_room: Some(format!("room-{name}").into()),
                    members: Vec::new(),
                    points,
                });
            }
            ctx.ready();
            while let Some(cmd) = ctx.next_command().await {
                if &*cmd.key == PRESS {
                    ctx.pulse_state(&cmd.device.id, PRESS, cmd.value.clone());
                } else if &*cmd.key == "gone" {
                    ctx.set_availability(&cmd.device.id, cmd.value != Value::Bool(true));
                } else {
                    let native = cmd.device.id.as_str().split_once(':').unwrap().1;
                    self.orders
                        .lock()
                        .unwrap()
                        .push((format!("{native}/{}", cmd.key), cmd.value.clone()));
                    if &*cmd.key == PERCENT {
                        ctx.set_state(&cmd.device.id, PERCENT, cmd.value.clone());
                    }
                }
                cmd.reply(Ok(()));
            }
            Ok(())
        })
    }
}

struct House {
    hub: Hub,
    groups: Groups,
    fake: Arc<Fake>,
}

async fn house(data_dir: Option<&std::path::Path>) -> House {
    let hub = Hub::new(HubOptions::default()).unwrap();
    let fake = Arc::new(Fake::default());
    let cancel = CancellationToken::new();
    spawn_driver(&hub, InstanceId::from("fake"), fake.clone(), cancel.clone());
    until(|| hub.device(&DeviceId::from("fake:lamp")).is_some()).await;
    let groups = Groups::open(hub.clone(), data_dir);
    spawn_driver(
        &hub,
        InstanceId::from(INSTANCE),
        Arc::new(Covers::new(hub.clone(), groups.clone())),
        cancel,
    );
    House { hub, groups, fake }
}

async fn until(mut what: impl FnMut() -> bool) {
    for _ in 0..500 {
        if what() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never came true");
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| format!("fake:{n}")).collect()
}

fn state(hub: &Hub, point: &str) -> Option<Value> {
    hub.state(&PointId::from(point)).map(|s| s.value)
}

async fn set(hub: &Hub, point: &str, value: Value) {
    hub.command(&PointId::from(point), value, Origin::System, None)
        .await
        .unwrap();
}

/// The orders the shutters got, in a fixed order (they leave side by side).
fn given(fake: &Fake) -> Vec<(String, Value)> {
    let mut orders = std::mem::take(&mut *fake.orders.lock().unwrap());
    orders.sort_by(|a, b| a.0.cmp(&b.0));
    orders
}

#[tokio::test]
async fn a_group_is_a_shutter_for_everyone() {
    let h = house(None).await;
    let group = h
        .groups
        .create(" Pièce  de vie ", &ids(&["a", "b"]))
        .unwrap();
    assert_eq!(group.name, "Pièce de vie");
    let id = group.device_id();
    until(|| h.hub.device(&id).is_some()).await;
    let view = h.hub.device(&id).unwrap();
    assert!(is_cover(&view.device));
    assert_eq!(view.device.model.as_deref(), Some(MODEL));
    assert_eq!(
        view.device.members,
        [DeviceId::from("fake:a"), DeviceId::from("fake:b")]
    );
    // The guard's rooms: its members'.
    let mut rooms = h.hub.rooms_of(&id);
    rooms.sort();
    assert_eq!(rooms, ["room-a", "room-b"]);
}

#[tokio::test]
async fn orders_reach_every_member_and_the_position_is_their_mean() {
    let h = house(None).await;
    let group = h.groups.create("Vie", &ids(&["a", "b"])).unwrap();
    let id = group.device_id();
    let at = format!("{id}/{PERCENT}");
    until(|| h.hub.device(&id).is_some()).await;

    set(&h.hub, &format!("{id}/{CONTROL}"), Value::from("open")).await;
    assert_eq!(
        given(&h.fake),
        [
            ("a/control".to_owned(), Value::from("open")),
            ("b/control".to_owned(), Value::from("open"))
        ]
    );
    set(&h.hub, &at, Value::Int(40)).await;
    assert_eq!(
        given(&h.fake),
        [
            ("a/percent_control".to_owned(), Value::Float(40.0)),
            ("b/percent_control".to_owned(), Value::Float(40.0))
        ]
    );
    until(|| state(&h.hub, &at) == Some(Value::Int(40))).await;
    // Members that differ: their mean, rounded.
    set(&h.hub, "fake:a/percent_control", Value::Int(100)).await;
    set(&h.hub, "fake:b/percent_control", Value::Int(1)).await;
    until(|| state(&h.hub, &at) == Some(Value::Int(51))).await;
}

#[tokio::test]
async fn a_member_out_of_reach_is_left_out_of_orders_and_of_the_mean() {
    let h = house(None).await;
    let group = h.groups.create("Vie", &ids(&["a", "b"])).unwrap();
    let id = group.device_id();
    let at = format!("{id}/{PERCENT}");
    until(|| h.hub.device(&id).is_some()).await;
    set(&h.hub, "fake:a/percent_control", Value::Int(100)).await;
    set(&h.hub, "fake:b/percent_control", Value::Int(0)).await;
    until(|| state(&h.hub, &at) == Some(Value::Int(50))).await;
    given(&h.fake);

    set(&h.hub, "fake:b/gone", Value::Bool(true)).await;
    until(|| state(&h.hub, &at) == Some(Value::Int(100))).await;
    assert_eq!(
        state(&h.hub, &format!("{id}/{UNREACHABLE}")),
        Some(Value::Int(1))
    );
    assert_ne!(h.hub.device(&id).unwrap().online, Some(false));
    set(&h.hub, &format!("{id}/{CONTROL}"), Value::from("close")).await;
    assert_eq!(
        given(&h.fake),
        [("a/control".to_owned(), Value::from("close"))]
    );

    // Nobody left: offline, its last position kept, and an order is refused.
    set(&h.hub, "fake:a/gone", Value::Bool(true)).await;
    until(|| h.hub.device(&id).unwrap().online == Some(false)).await;
    assert_eq!(state(&h.hub, &at), Some(Value::Int(100)));
    // The hub itself refuses an order to a device that is offline, or the
    // group does: either way, no shutter is told anything.
    let refused = h
        .hub
        .command(
            &PointId::from(format!("{id}/{CONTROL}").as_str()),
            Value::from("open"),
            Origin::System,
            None,
        )
        .await;
    assert!(refused.is_err());
    assert!(given(&h.fake).is_empty());
}

#[tokio::test]
async fn a_shutter_can_be_in_several_groups() {
    let h = house(None).await;
    let one = h.groups.create("Un", &ids(&["a", "b"])).unwrap();
    let two = h.groups.create("Deux", &ids(&["b", "c"])).unwrap();
    assert_ne!(one.id, two.id);
    until(|| h.hub.device(&two.device_id()).is_some()).await;
    set(&h.hub, "fake:b/percent_control", Value::Int(30)).await;
    let (one_at, two_at) = (
        format!("{}/{PERCENT}", one.device_id()),
        format!("{}/{PERCENT}", two.device_id()),
    );
    until(|| {
        state(&h.hub, &one_at) == Some(Value::Int(30))
            && state(&h.hub, &two_at) == Some(Value::Int(30))
    })
    .await;
    set(
        &h.hub,
        &format!("{}/{CONTROL}", two.device_id()),
        Value::from("stop"),
    )
    .await;
    let names: Vec<String> = given(&h.fake).into_iter().map(|(p, _)| p).collect();
    assert!(names.contains(&"c/control".to_owned()));
    assert!(!names.contains(&"a/control".to_owned()));
}

#[tokio::test]
async fn editing_a_group_changes_its_device_not_its_id() {
    let h = house(None).await;
    let group = h.groups.create("Vie", &ids(&["a"])).unwrap();
    let id = group.device_id();
    until(|| h.hub.device(&id).is_some()).await;
    let edited = h
        .groups
        .update(&group.id, "Séjour", &ids(&["b", "c"]))
        .unwrap();
    assert_eq!(edited.device_id(), id);
    until(|| &*h.hub.device(&id).unwrap().device.native_name == "Séjour").await;
    assert_eq!(h.hub.device(&id).unwrap().device.members.len(), 2);
    assert!(matches!(
        h.groups.update("g99", "X", &ids(&["a"])),
        Err(GroupError::UnknownGroup)
    ));
    h.groups.remove(&group.id).unwrap();
    until(|| h.hub.device(&id).is_none()).await;
    assert!(matches!(
        h.groups.remove(&group.id),
        Err(GroupError::UnknownGroup)
    ));
}

#[tokio::test]
async fn what_makes_no_group_is_refused() {
    let h = house(None).await;
    let invalid = |r: Result<Group, GroupError>| matches!(r, Err(GroupError::Invalid(_)));
    assert!(invalid(h.groups.create("  ", &ids(&["a"]))));
    assert!(invalid(
        h.groups.create(&"x".repeat(MAX_NAME + 1), &ids(&["a"]))
    ));
    assert!(invalid(h.groups.create("Vide", &[])));
    assert!(invalid(h.groups.create("Lampe", &ids(&["lamp"]))));
    assert!(invalid(h.groups.create("Inconnu", &ids(&["nope"]))));
    // Twice the same shutter is once.
    let group = h.groups.create("Vie", &ids(&["a", "a"])).unwrap();
    assert_eq!(group.members.len(), 1);
    // No group in a group.
    let id = group.device_id();
    until(|| h.hub.device(&id).is_some()).await;
    let nested = [id.as_str().to_owned()];
    assert!(invalid(h.groups.create("Groupe de groupes", &nested)));
    assert!(invalid(h.groups.update(&group.id, "Soi-même", &nested)));
}

#[tokio::test]
async fn groups_outlive_a_restart_and_a_deleted_id_is_never_used_again() {
    let dir = std::env::temp_dir().join(format!("moli-covers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    {
        let h = house(Some(&dir)).await;
        let kept = h.groups.create("Vie", &ids(&["a", "b"])).unwrap();
        let gone = h.groups.create("Passage", &ids(&["c"])).unwrap();
        assert_eq!((kept.id.as_str(), gone.id.as_str()), ("g1", "g2"));
        h.groups.remove(&gone.id).unwrap();
    }
    let h = house(Some(&dir)).await;
    let groups = h.groups.list();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, "g1");
    until(|| h.hub.device(&groups[0].device_id()).is_some()).await;
    let new = h.groups.create("Nouveau", &ids(&["c"])).unwrap();
    assert_eq!(new.id, "g3");

    // A shutter whose driver is away stays in its group, for now.
    let mut members = ids(&["a"]);
    members.push("ailleurs:volet".to_owned());
    assert!(h.groups.update("g1", "Vie", &members).is_err());
    std::fs::write(
        dir.join("cover-groups.json"),
        r#"{"next":3,"groups":[{"id":"g1","name":"Vie","members":["ailleurs:volet"]}]}"#,
    )
    .unwrap();
    let again = Groups::open(h.hub.clone(), Some(&dir));
    assert!(
        again
            .update("g1", "Vie", &["ailleurs:volet".to_owned()])
            .is_ok()
    );

    // A damaged file is set aside, not overwritten.
    std::fs::write(dir.join("cover-groups.json"), "{ not json").unwrap();
    let damaged = Groups::open(h.hub.clone(), Some(&dir));
    assert!(damaged.list().is_empty());
    assert!(dir.join("cover-groups.json.bad").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_member_that_only_opens_goes_to_the_nearest_end() {
    let device = Device {
        id: DeviceId::from("fake:d"),
        instance: InstanceId::from("fake"),
        native_name: "d".into(),
        manufacturer: None,
        model: None,
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![control()],
    };
    let k = keys(&device).unwrap();
    assert_eq!(
        order_for(&k, Order::To(70.0)),
        Some((CONTROL, Value::from("open")))
    );
    assert_eq!(
        order_for(&k, Order::To(30.0)),
        Some((CONTROL, Value::from("close")))
    );
}

#[test]
fn a_member_with_its_own_range_is_read_and_told_in_percent() {
    let mut spec = percent();
    spec.kind = Kind::Numeric {
        min: Some(0.0),
        max: Some(254.0),
        step: None,
    };
    let device = Device {
        id: DeviceId::from("fake:e"),
        instance: InstanceId::from("fake"),
        native_name: "e".into(),
        manufacturer: None,
        model: None,
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![spec],
    };
    let k = keys(&device).unwrap();
    assert_eq!(
        order_for(&k, Order::To(50.0)),
        Some((PERCENT, Value::Float(127.0)))
    );
    // It cannot stop: a stop is nothing to it.
    assert_eq!(order_for(&k, Order::Stop), None);
    assert_eq!(
        order_for(&k, Order::Open),
        Some((PERCENT, Value::Float(254.0)))
    );
}

#[test]
fn what_a_group_shows() {
    let m = |answers, position, slats_open| Member {
        answers,
        position,
        slats_open,
    };
    assert_eq!(
        shown(&[m(true, Some(100.0), false), m(true, Some(0.0), false)]),
        Shown {
            online: true,
            position: Some(50),
            slats_open: false,
            unreachable: 0,
        }
    );
    // One knows, one does not, one is away with a last position.
    assert_eq!(
        shown(&[
            m(true, Some(20.0), true),
            m(true, None, false),
            m(false, Some(100.0), true),
        ]),
        Shown {
            online: true,
            position: Some(20),
            slats_open: true,
            unreachable: 1,
        }
    );
    assert_eq!(
        shown(&[m(false, Some(100.0), false)]),
        Shown {
            online: false,
            position: None,
            slats_open: false,
            unreachable: 1,
        }
    );
    assert_eq!(
        shown(&[m(true, None, false)]),
        Shown {
            online: true,
            position: None,
            slats_open: false,
            unreachable: 0,
        }
    );
}

fn switch(open: Option<&str>, close: Option<&str>, stop: Option<&str>) -> Switch {
    Switch {
        device: "fake:switch".into(),
        open: open.map(Into::into),
        close: close.map(Into::into),
        stop: stop.map(Into::into),
    }
}

#[tokio::test]
async fn a_switch_commands_its_groups_press_after_press() {
    let h = house(None).await;
    let left = h
        .groups
        .create_with(
            "Vie",
            &ids(&["a"]),
            &[switch(Some("on_top"), Some("off_top"), None)],
        )
        .unwrap();
    let right = h
        .groups
        .create_with(
            "Tout",
            &ids(&["b"]),
            &[switch(Some("on_bottom"), None, None)],
        )
        .unwrap();
    until(|| {
        h.hub.device(&left.device_id()).is_some() && h.hub.device(&right.device_id()).is_some()
    })
    .await;

    let press = "fake:switch/action";
    set(&h.hub, press, Value::from("on_top")).await;
    until(|| !h.fake.orders.lock().unwrap().is_empty()).await;
    assert_eq!(
        given(&h.fake),
        [("a/control".to_owned(), Value::from("open"))]
    );
    // The same button again is another press.
    set(&h.hub, press, Value::from("on_top")).await;
    until(|| !h.fake.orders.lock().unwrap().is_empty()).await;
    assert_eq!(
        given(&h.fake),
        [("a/control".to_owned(), Value::from("open"))]
    );
    set(&h.hub, press, Value::from("off_top")).await;
    until(|| !h.fake.orders.lock().unwrap().is_empty()).await;
    assert_eq!(
        given(&h.fake),
        [("a/control".to_owned(), Value::from("close"))]
    );
    set(&h.hub, press, Value::from("on_bottom")).await;
    until(|| !h.fake.orders.lock().unwrap().is_empty()).await;
    assert_eq!(
        given(&h.fake),
        [("b/control".to_owned(), Value::from("open"))]
    );
}

#[tokio::test]
async fn switches_are_checked_and_kept_unless_given() {
    let h = house(None).await;
    let invalid = |r: Result<Group, GroupError>| matches!(r, Err(GroupError::Invalid(_)));
    // No press, the same press twice, a lamp, the same switch twice.
    assert!(invalid(h.groups.create_with(
        "X",
        &ids(&["a"]),
        &[switch(None, None, None)]
    )));
    assert!(invalid(h.groups.create_with(
        "X",
        &ids(&["a"]),
        &[switch(Some("on_top"), Some("on_top"), None)]
    )));
    let mut lamp = switch(Some("on_top"), None, None);
    lamp.device = "fake:lamp".into();
    assert!(invalid(h.groups.create_with("X", &ids(&["a"]), &[lamp])));
    let one = switch(Some("on_top"), None, None);
    assert!(invalid(h.groups.create_with(
        "X",
        &ids(&["a"]),
        &[one.clone(), one.clone()]
    )));

    let group = h
        .groups
        .create_with(
            "Vie",
            &ids(&["a"]),
            &[switch(Some(" on_top "), Some(""), None)],
        )
        .unwrap();
    assert_eq!(group.switches, std::slice::from_ref(&one));
    let renamed = h.groups.update(&group.id, "Séjour", &ids(&["a"])).unwrap();
    assert_eq!(renamed.switches, [one]);
    let cleared = h
        .groups
        .update_with(&group.id, "Séjour", &ids(&["a"]), Some(&[]))
        .unwrap();
    assert!(cleared.switches.is_empty());
}
