//! Picture settings: how big the stream is, how many frames a second, and
//! whether to save battery (fewer bits). A default in Settings; each device
//! may have its own. The defaults match DeskSpan before this setting existed
//! (30 fps, very wide screens halved, about 8 Mbit/s at 1080p).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// Up to 4K wide before shrinking: sharpest text, most work.
    Full,
    /// Up to 2560 wide (wider is shrunk).
    Balanced,
    /// Up to 1600 wide: lightest on Wi-Fi and batteries.
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quality {
    pub size: Size,
    /// 30, 60 or 120 (a ceiling: big screens may manage fewer).
    pub fps: u32,
    /// Fewer bits per frame: less work for both devices.
    pub saver: bool,
}

impl Default for Quality {
    fn default() -> Self {
        Quality {
            size: Size::Balanced,
            fps: 30,
            saver: false,
        }
    }
}

impl Quality {
    /// The frame rate, snapped to one DeskSpan offers.
    pub fn fps(&self) -> u32 {
        match self.fps {
            0..=44 => 30,
            45..=89 => 60,
            _ => 120,
        }
    }

    /// Streams wider than this are shrunk (by a whole number) before encoding.
    pub fn max_width(&self) -> u32 {
        match self.size {
            Size::Full => 3840,
            Size::Balanced => 2560,
            Size::Light => 1600,
        }
    }

    /// The whole-number shrink for a screen `w` pixels wide.
    pub fn shrink(&self, w: u32) -> u32 {
        w.div_ceil(self.max_width()).max(1)
    }

    /// Bits per second for a `w`×`h` stream: about 0.13 bit per pixel per
    /// frame (8 Mbit/s at 1080p30), half that with the battery saver.
    pub fn bitrate(&self, w: u32, h: u32) -> u32 {
        let per_second = f64::from(w) * f64::from(h) * f64::from(self.fps());
        let (factor, low, high) = if self.saver {
            (0.065, 2_000_000.0, 12_000_000.0)
        } else {
            (0.13, 4_000_000.0, 40_000_000.0)
        };
        (per_second * factor).clamp(low, high) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_old_fixed_values() {
        let q = Quality::default();
        assert_eq!(q.fps(), 30);
        assert_eq!(q.max_width(), 2560);
        let b = q.bitrate(1920, 1080);
        assert!((7_500_000..=8_500_000).contains(&b), "{b}");
        // An old settings file without the field reads as the default.
        let back: Quality = serde_json::from_str("{}").unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn frame_rates_snap_and_bitrates_stay_in_range() {
        let q = |fps| Quality {
            fps,
            ..Quality::default()
        };
        assert_eq!(q(0).fps(), 30);
        assert_eq!(q(59).fps(), 60);
        assert_eq!(q(500).fps(), 120);
        assert_eq!(q(30).bitrate(320, 240), 4_000_000);
        assert_eq!(q(120).bitrate(3840, 2160), 40_000_000);
        let saver = Quality {
            saver: true,
            ..Quality::default()
        };
        assert!(saver.bitrate(1920, 1080) < Quality::default().bitrate(1920, 1080));
    }

    #[test]
    fn wide_screens_shrink_by_whole_numbers() {
        let full = Quality {
            size: Size::Full,
            ..Quality::default()
        };
        let light = Quality {
            size: Size::Light,
            ..Quality::default()
        };
        assert_eq!(Quality::default().shrink(2560), 1);
        assert_eq!(Quality::default().shrink(5120), 2);
        assert_eq!(full.shrink(5120), 2);
        assert_eq!(full.shrink(3840), 1);
        assert_eq!(light.shrink(2560), 2);
        assert_eq!(light.shrink(5120), 4);
        let json = serde_json::to_string(&light).unwrap();
        assert!(json.contains("\"light\""));
    }
}
