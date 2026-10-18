//! Devices the user has allowed. Each gets an id and a secret key, kept by
//! the receiver page (its browser storage); only the key's SHA-256 is stored
//! here, in devices.json.

use crate::secret;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    name: String,
    key_hash: String,
    /// Unix seconds
    added: u64,
    last_seen: u64,
    /// May send touch, pen, mouse and keys to this computer.
    control: bool,
}

/// What the app's window shows (never the key).
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub added: u64,
    pub last_seen: u64,
    pub control: bool,
}

pub struct Devices {
    path: PathBuf,
    list: Vec<Stored>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Devices {
    pub fn load(dir: &Path) -> Devices {
        let path = dir.join("devices.json");
        let list = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("{} is not valid, starting empty: {e}", path.display());
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!("cannot read {}: {e}", path.display());
                Vec::new()
            }
        };
        Devices { path, list }
    }

    fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.list).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("cannot write the device list: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot save the device list: {e}"))
    }

    pub fn list(&self) -> Vec<Device> {
        self.list
            .iter()
            .map(|d| Device {
                id: d.id.clone(),
                name: d.name.clone(),
                added: d.added,
                last_seen: d.last_seen,
                control: d.control,
            })
            .collect()
    }

    /// A newly allowed device: its id and the key it must keep.
    pub fn add(&mut self, name: &str, control: bool) -> Result<(String, String), String> {
        let id = secret::random_hex(8)?;
        let key = secret::random_hex(32)?;
        let t = now();
        self.list.push(Stored {
            id: id.clone(),
            name: clean_name(name),
            key_hash: secret::sha256_hex(&key),
            added: t,
            last_seen: t,
            control,
        });
        self.save()?;
        Ok((id, key))
    }

    /// The device, if `key` is its key; its "last seen" is updated.
    pub fn check(&mut self, id: &str, key: &str) -> Option<Device> {
        let hash = secret::sha256_hex(key);
        let d = self
            .list
            .iter_mut()
            .find(|d| d.id == id && secret::same(&d.key_hash, &hash))?;
        d.last_seen = now();
        let out = Device {
            id: d.id.clone(),
            name: d.name.clone(),
            added: d.added,
            last_seen: d.last_seen,
            control: d.control,
        };
        if let Err(e) = self.save() {
            tracing::warn!("{e}");
        }
        Some(out)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.list.retain(|d| d.id != id);
        self.save()
    }

    pub fn set_control(&mut self, id: &str, control: bool) -> Result<(), String> {
        let d = self
            .list
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or("that device is no longer in the list")?;
        d.control = control;
        self.save()
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let d = self
            .list
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or("that device is no longer in the list")?;
        d.name = clean_name(name);
        self.save()
    }
}

/// A device names itself: keep it short and printable.
pub fn clean_name(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let n = n.trim();
    if n.is_empty() {
        "Unnamed device".into()
    } else {
        n.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_check_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Devices::load(dir.path());
        let (id, key) = d.add("iPad", false).unwrap();
        assert!(d.check(&id, &key).is_some());
        assert!(d.check(&id, "wrong").is_none());
        // Survives a restart, and the key itself isn't in the file.
        let text = std::fs::read_to_string(dir.path().join("devices.json")).unwrap();
        assert!(!text.contains(&key));
        let mut again = Devices::load(dir.path());
        assert_eq!(again.list()[0].name, "iPad");
        again.set_control(&id, true).unwrap();
        assert!(again.check(&id, &key).unwrap().control);
        again.remove(&id).unwrap();
        assert!(again.list().is_empty());
    }

    #[test]
    fn names_are_tidied() {
        assert_eq!(clean_name("  Pixel\n 8 "), "Pixel 8");
        assert_eq!(clean_name("\u{7}"), "Unnamed device");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 60);
    }
}
