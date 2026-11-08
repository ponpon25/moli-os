//! The groups as the family made them, kept in `cover-groups.json`.
//!
//! A group's id is never used twice: Maison remembers an accessory by the
//! `aid` Moli gave its device, so a new group must not inherit what a deleted
//! one left behind (its place in a room, its automations).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use moli_core::DeviceId;
use moli_runtime::Hub;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::{INSTANCE, is_cover};

/// Longest name of a group (as a device label).
pub const MAX_NAME: usize = 60;
/// Most shutters in one group.
pub const MAX_MEMBERS: usize = 32;
/// Most groups in a house.
pub const MAX_GROUPS: usize = 64;
/// Most switches commanding one group.
pub const MAX_SWITCHES: usize = 8;
/// The point a switch's presses arrive on (Zigbee remotes: `action`).
pub const PRESS: &str = "action";
/// Longest press name kept.
const MAX_PRESS: usize = 64;

/// One group of shutters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// Stable: the device is `groupes-stores:<id>`; a rename keeps it.
    pub id: String,
    pub name: String,
    /// Device ids. A shutter may be in any number of groups.
    pub members: Vec<String>,
    /// Wall switches that command the group, press by press.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<Switch>,
}

/// A real switch that commands a group: which of its presses (values of its
/// [`PRESS`] point) opens, closes or stops it. A switch may serve several
/// groups (left side one, right side another).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    /// Its device id.
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
}

impl Switch {
    /// The presses it uses, set ones only.
    fn presses(&self) -> impl Iterator<Item = &String> {
        [&self.open, &self.close, &self.stop].into_iter().flatten()
    }
}

impl Group {
    /// The device that stands for this group.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from(format!("{INSTANCE}:{}", self.id))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    /// The number the next group is given.
    #[serde(default)]
    next: u64,
    #[serde(default)]
    groups: Vec<Group>,
}

/// Why a change to the groups was refused.
#[derive(Debug)]
pub enum GroupError {
    /// What was asked makes no group; the text says why.
    Invalid(String),
    UnknownGroup,
    Io(std::io::Error),
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => f.write_str(why),
            Self::UnknownGroup => f.write_str(&moli_i18n::tr!("pilotes.covers.groupe_inconnu")),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for GroupError {}

/// The house's groups, shared by the driver and the API.
#[derive(Clone, Debug)]
pub struct Groups(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    hub: Hub,
    path: Option<PathBuf>,
    saved: Mutex<Saved>,
    /// Moves on with each change; the driver follows it.
    changes: watch::Sender<u64>,
}

/// A damaged file is set aside, never overwritten by the next change.
fn load(path: &Path) -> Saved {
    let Ok(bytes) = std::fs::read(path) else {
        return Saved::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(saved) => saved,
        Err(e) => {
            let aside = path.with_extension("json.bad");
            let kept = std::fs::rename(path, &aside).is_ok();
            tracing::error!(error = %e, kept, "cover groups: file unreadable, set aside");
            Saved::default()
        }
    }
}

/// One line, single spaces, no control characters.
fn clean_name(raw: &str) -> Result<String, GroupError> {
    let name = raw
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(GroupError::Invalid(moli_i18n::tr!(
            "pilotes.covers.nom_attendu",
            max = MAX_NAME
        )));
    }
    Ok(name)
}

impl Groups {
    /// Reads `cover-groups.json` from `data_dir`; none: nothing outlives a
    /// restart (tests).
    #[must_use]
    pub fn open(hub: Hub, data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|d| d.join("cover-groups.json"));
        let saved = path.as_deref().map(load).unwrap_or_default();
        Self(Arc::new(Inner {
            hub,
            path,
            saved: Mutex::new(saved),
            changes: watch::channel(0).0,
        }))
    }

    fn saved(&self) -> std::sync::MutexGuard<'_, Saved> {
        self.0.saved.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn list(&self) -> Vec<Group> {
        self.saved().groups.clone()
    }

    pub(crate) fn watch(&self) -> watch::Receiver<u64> {
        self.0.changes.subscribe()
    }

    /// Every member a shutter Moli knows, once each; those already in the
    /// group may stay while their driver is away.
    fn members(&self, already: &[String], asked: &[String]) -> Result<Vec<String>, GroupError> {
        let mut members: Vec<String> = Vec::with_capacity(asked.len());
        for id in asked {
            if members.contains(id) {
                continue;
            }
            let device = DeviceId::from(id.as_str());
            let shutter = !id.starts_with(&format!("{INSTANCE}:"))
                && self
                    .0
                    .hub
                    .device(&device)
                    .is_some_and(|v| is_cover(&v.device));
            if !shutter && !already.contains(id) {
                return Err(GroupError::Invalid(moli_i18n::tr!(
                    "pilotes.covers.pas_un_store",
                    id = id
                )));
            }
            members.push(id.clone());
        }
        if members.is_empty() || members.len() > MAX_MEMBERS {
            return Err(GroupError::Invalid(moli_i18n::tr!(
                "pilotes.covers.membres_attendus",
                max = MAX_MEMBERS
            )));
        }
        Ok(members)
    }

    /// Switches Moli knows (those already set may stay while their driver
    /// is away), each with at least one press, no press twice.
    fn switches(&self, already: &[Switch], asked: &[Switch]) -> Result<Vec<Switch>, GroupError> {
        let invalid =
            || GroupError::Invalid(moli_i18n::tr!("pilotes.covers.interrupteur_invalide"));
        if asked.len() > MAX_SWITCHES {
            return Err(invalid());
        }
        let mut kept: Vec<Switch> = Vec::with_capacity(asked.len());
        for switch in asked {
            let clean = |p: &Option<String>| {
                p.as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
            };
            let switch = Switch {
                device: switch.device.clone(),
                open: clean(&switch.open),
                close: clean(&switch.close),
                stop: clean(&switch.stop),
            };
            let presses: Vec<&String> = switch.presses().collect();
            let distinct = presses
                .iter()
                .enumerate()
                .all(|(i, p)| !presses[..i].contains(p));
            if presses.is_empty()
                || !distinct
                || presses.iter().any(|p| p.chars().count() > MAX_PRESS)
                || kept.iter().any(|k| k.device == switch.device)
            {
                return Err(invalid());
            }
            let known = self
                .0
                .hub
                .device(&DeviceId::from(switch.device.as_str()))
                .is_some_and(|v| v.device.point(PRESS).is_some());
            if !known && !already.iter().any(|a| a.device == switch.device) {
                return Err(GroupError::Invalid(moli_i18n::tr!(
                    "pilotes.covers.pas_un_interrupteur",
                    id = switch.device
                )));
            }
            kept.push(switch);
        }
        Ok(kept)
    }

    /// Written aside then renamed: never half a file. Memory follows only
    /// once it is on disk.
    fn commit(&self, saved: &mut Saved, next: Saved) -> Result<(), GroupError> {
        if let Some(path) = &self.0.path {
            let bytes = serde_json::to_vec_pretty(&next)
                .map_err(|e| GroupError::Io(std::io::Error::other(e)))?;
            let aside = path.with_extension("json.new");
            std::fs::write(&aside, bytes).map_err(GroupError::Io)?;
            std::fs::rename(&aside, path).map_err(GroupError::Io)?;
        }
        *saved = next;
        self.0.changes.send_modify(|n| *n += 1);
        Ok(())
    }

    pub fn create(&self, name: &str, members: &[String]) -> Result<Group, GroupError> {
        self.create_with(name, members, &[])
    }

    /// Like [`Self::create`], with the switches that command it.
    pub fn create_with(
        &self,
        name: &str,
        members: &[String],
        switches: &[Switch],
    ) -> Result<Group, GroupError> {
        let name = clean_name(name)?;
        let members = self.members(&[], members)?;
        let switches = self.switches(&[], switches)?;
        let mut saved = self.saved();
        if saved.groups.len() >= MAX_GROUPS {
            return Err(GroupError::Invalid(moli_i18n::tr!(
                "pilotes.covers.trop_de_groupes",
                max = MAX_GROUPS
            )));
        }
        let mut next = saved.clone();
        next.next += 1;
        let group = Group {
            id: format!("g{}", next.next),
            name,
            members,
            switches,
        };
        next.groups.push(group.clone());
        self.commit(&mut saved, next)?;
        Ok(group)
    }

    pub fn update(&self, id: &str, name: &str, members: &[String]) -> Result<Group, GroupError> {
        self.update_with(id, name, members, None)
    }

    /// Like [`Self::update`]; `switches`: `None` keeps those it has.
    pub fn update_with(
        &self,
        id: &str,
        name: &str,
        members: &[String],
        switches: Option<&[Switch]>,
    ) -> Result<Group, GroupError> {
        let name = clean_name(name)?;
        let mut saved = self.saved();
        let at = saved
            .groups
            .iter()
            .position(|g| g.id == id)
            .ok_or(GroupError::UnknownGroup)?;
        let members = self.members(&saved.groups[at].members, members)?;
        let before = &saved.groups[at].switches;
        let switches = match switches {
            Some(asked) => self.switches(before, asked)?,
            None => before.clone(),
        };
        let mut next = saved.clone();
        let group = Group {
            id: id.to_owned(),
            name,
            members,
            switches,
        };
        next.groups[at] = group.clone();
        self.commit(&mut saved, next)?;
        Ok(group)
    }

    pub fn remove(&self, id: &str) -> Result<Group, GroupError> {
        let mut saved = self.saved();
        let at = saved
            .groups
            .iter()
            .position(|g| g.id == id)
            .ok_or(GroupError::UnknownGroup)?;
        let mut next = saved.clone();
        let gone = next.groups.remove(at);
        self.commit(&mut saved, next)?;
        Ok(gone)
    }
}
