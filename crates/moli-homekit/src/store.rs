//! What the bridge keeps on disk (`data/homekit.json`): its identifiers, the
//! controllers it is paired with (public keys only), which devices are shown,
//! the identifiers given to accessories. No secret here: the setup code and
//! the bridge's private key live in Moli's encrypted store.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::accessories::Entry;

/// Which devices HomeKit sees unless told otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// Every device HomeKit has a place for, except those switched off.
    #[default]
    All,
    /// None, except those switched on.
    None,
}

impl Policy {
    pub fn exposes(self) -> bool {
        self == Self::All
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// The controller's pairing identifier (a UUID).
    pub id: String,
    /// Its Ed25519 public key, hex.
    pub ltpk: String,
    pub admin: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    /// `AA:BB:CC:DD:EE:FF`, the bridge's pairing identifier.
    pub device_id: String,
    pub setup_id: String,
    /// `c#`: raised each time the accessory database changes.
    pub config_number: u32,
    /// Hash of the database `config_number` stands for.
    pub structure: String,
    pub policy: Policy,
    /// Per-device choices that depart from the policy.
    pub exceptions: BTreeMap<String, bool>,
    /// Accessory identifiers, kept for good.
    pub aids: BTreeMap<String, u64>,
    pub next_aid: u64,
    pub pairings: Vec<Pairing>,
    /// The last description of each exposed device: the database does not
    /// shrink while drivers are still starting.
    pub devices: BTreeMap<String, Entry>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            device_id: crate::setup::new_device_id(),
            setup_id: crate::setup::new_setup_id(),
            config_number: 1,
            structure: String::new(),
            policy: Policy::default(),
            exceptions: BTreeMap::new(),
            aids: BTreeMap::new(),
            next_aid: 2,
            pairings: Vec::new(),
            devices: BTreeMap::new(),
        }
    }
}

impl Saved {
    /// Reads the file; a missing file is a fresh bridge. A damaged file is
    /// an error: starting over would silently unpair every phone.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Atomically, readable by the owner only.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_private(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)
    }

    pub fn exposed(&self, id: &str) -> bool {
        self.exceptions
            .get(id)
            .copied()
            .unwrap_or_else(|| self.policy.exposes())
    }

    pub fn paired(&self) -> bool {
        !self.pairings.is_empty()
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("moli-homekit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("homekit.json")
    }

    #[test]
    fn a_missing_file_is_a_fresh_bridge_with_its_own_identity() {
        let path = temp("missing");
        let a = Saved::load(&path).unwrap();
        let b = Saved::load(&path).unwrap();
        assert_ne!(a.device_id, b.device_id);
        assert_eq!(a.config_number, 1);
        assert!(!a.paired());
        assert_eq!(a.policy, Policy::All);
    }

    #[test]
    fn what_is_saved_comes_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = temp("roundtrip");
        let mut saved = Saved {
            policy: Policy::None,
            ..Saved::default()
        };
        saved.exceptions.insert("hue:abc".into(), true);
        saved.pairings.push(Pairing {
            id: "ID".into(),
            ltpk: "00ff".into(),
            admin: true,
        });
        saved.save(&path).unwrap();
        let back = Saved::load(&path).unwrap();
        assert_eq!(back.device_id, saved.device_id);
        assert_eq!(back.pairings, saved.pairings);
        assert!(back.exposed("hue:abc"));
        assert!(!back.exposed("hue:other"));
        assert!(back.paired());
        // Private to its owner.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn exceptions_override_the_policy_both_ways() {
        let mut saved = Saved::default();
        assert!(saved.exposed("x"));
        saved.exceptions.insert("x".into(), false);
        assert!(!saved.exposed("x"));
        saved.policy = Policy::None;
        saved.exceptions.clear();
        assert!(!saved.exposed("x"));
        saved.exceptions.insert("x".into(), true);
        assert!(saved.exposed("x"));
    }

    #[test]
    fn a_damaged_file_is_refused_not_replaced() {
        let path = temp("damaged");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Saved::load(&path).is_err());
    }
}
