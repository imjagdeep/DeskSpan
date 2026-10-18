//! DeskSpan's settings, kept as settings.json in the app's data folder.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The port receivers connect to unless the user picks another.
pub const DEFAULT_PORT: u16 = 47800;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// TCP port of the receiver page and signalling.
    pub port: u16,
    /// Off: only home/office network addresses and this computer. On: every
    /// network, including VPNs like Tailscale.
    pub listen_everywhere: bool,
    /// Start sharing as soon as DeskSpan starts (trusted devices reconnect).
    pub start_on_launch: bool,
    /// Set after the first start, when "start with the computer" was offered.
    pub autostart_applied: bool,
    /// Android phones by USB cable (adb) were set up.
    pub usb_cable: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            port: DEFAULT_PORT,
            listen_everywhere: false,
            start_on_launch: false,
            autostart_applied: false,
            usb_cable: false,
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("{} is not valid, using defaults: {e}", path.display());
                Settings::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                tracing::warn!("cannot read {}: {e}", path.display());
                Settings::default()
            }
        }
    }

    /// Write through a temp file so a crash can't leave half a file.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("cannot write settings: {e}"))?;
        std::fs::rename(&tmp, dir.join("settings.json"))
            .map_err(|e| format!("cannot save settings: {e}"))
    }

    /// Ports below 1024 need admin rights on Linux/macOS.
    pub fn clamp(mut self) -> Settings {
        if self.port < 1024 {
            self.port = DEFAULT_PORT;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults_for_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).port, DEFAULT_PORT);
        let s = Settings {
            port: 50000,
            listen_everywhere: true,
            ..Settings::default()
        };
        s.save(dir.path()).unwrap();
        let back = Settings::load(dir.path());
        assert_eq!(back.port, 50000);
        assert!(back.listen_everywhere);
        std::fs::write(dir.path().join("settings.json"), r#"{"port":48000}"#).unwrap();
        let old = Settings::load(dir.path());
        assert_eq!(old.port, 48000);
        assert!(!old.start_on_launch);
    }

    #[test]
    fn low_ports_fall_back() {
        let s = Settings {
            port: 80,
            ..Settings::default()
        }
        .clamp();
        assert_eq!(s.port, DEFAULT_PORT);
    }
}
