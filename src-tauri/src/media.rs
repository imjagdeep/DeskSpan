//! Connects the pairing server to the picture: a WebRTC connection (peer.rs)
//! for every receiver that is let in, offered again if it drops while the
//! receiver is still there.
//!
//! Mirror receivers share the main screen's stream. Extend receivers get
//! their own virtual display (macOS, vdisplay.rs) and their own stream; on
//! systems that can't extend yet they are mirrored and told why.

use crate::peer::{self, Ended};
use crate::server::{Hub, Inbound, MediaEvent, Outbound, ScreenInfo};
use crate::video::Video;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;

/// What a newly joined receiver wants.
#[derive(Debug)]
pub struct Join {
    pub session: u64,
    pub device_id: String,
    pub name: String,
    pub screen: Option<ScreenInfo>,
    pub extend: bool,
}

/// Start listening for receivers; runs for the life of the app.
pub fn start(app: AppHandle, hub: Arc<Hub>, mirror: Video, data_dir: PathBuf) {
    let mut events = hub.take_signals();
    tauri::async_runtime::spawn(async move {
        let mut peers: HashMap<u64, mpsc::UnboundedSender<Inbound>> = HashMap::new();
        while let Some(ev) = events.recv().await {
            match ev {
                MediaEvent::Joined(join) => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    peers.insert(join.session, tx);
                    tauri::async_runtime::spawn(serve(
                        app.clone(),
                        hub.clone(),
                        join,
                        rx,
                        mirror.clone(),
                        data_dir.clone(),
                    ));
                }
                // Dropping the sender ends that receiver's connection.
                MediaEvent::Left { session } => {
                    peers.remove(&session);
                }
                MediaEvent::Signal { session, msg } => {
                    if let Some(tx) = peers.get(&session) {
                        let _ = tx.send(msg);
                    }
                }
            }
        }
    });
}

/// One receiver, for as long as it stays.
async fn serve(
    app: AppHandle,
    hub: Arc<Hub>,
    join: Join,
    mut signals: mpsc::UnboundedReceiver<Inbound>,
    mirror: Video,
    data_dir: PathBuf,
) {
    let session = join.session;
    // Extend: this receiver's own display and stream; otherwise the shared
    // main screen.
    let display = if join.extend {
        let s = join.screen.unwrap_or(ScreenInfo {
            w: 1280,
            h: 800,
            dpr: 2.0,
        });
        let plan = DisplayPlan::for_screen(s.w, s.h, s.dpr);
        let name = format!("DeskSpan – {}", join.name);
        match extend_display(&app, name, serial_for(&join.device_id), plan).await {
            Ok(id) => Some(id),
            Err(why) => {
                tracing::warn!("session {session}: no extra display: {why}");
                hub.send(
                    session,
                    &Outbound::Notice {
                        text: &format!("Showing a copy of the main screen: {why}"),
                    },
                );
                None
            }
        }
    } else {
        None
    };
    let video = match display {
        Some(id) => Video::for_display(data_dir, id),
        None => mirror,
    };

    let mut tries = 0u32;
    loop {
        match peer::run(&hub, session, &mut signals, &video).await {
            Ended::Left => break,
            Ended::Fatal(why) => {
                tracing::warn!("session {session}: {why}");
                hub.send(session, &Outbound::Notice { text: &why });
                // Nothing more to do until the receiver leaves.
                while signals.recv().await.is_some() {}
                break;
            }
            Ended::Dropped(why) => {
                tracing::info!("session {session}: {why}; offering again");
                tries += 1;
                // Back off a little, but keep trying while the receiver is there.
                let wait = Duration::from_millis(500 * u64::from(tries.min(10)));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    s = signals.recv() => if s.is_none() { break },
                }
            }
        }
    }
    if let Some(id) = display {
        remove_display(&app, id);
    }
}

/// A stable number per device: macOS remembers where each display was
/// arranged by its serial, so the device's screen comes back in the same
/// place next time.
fn serial_for(device_id: &str) -> u32 {
    // FNV-1a
    let mut h: u32 = 0x811c_9dc5;
    for b in device_id.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h.max(1)
}

/// What size to make an extend receiver's display.
// Read by vdisplay.rs, which is macOS-only until Windows/Linux extend lands.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayPlan {
    /// Desktop size in points (what windows are laid out in).
    pub points_w: u32,
    pub points_h: u32,
    /// 2 = HiDPI (Retina) backing, 1 = plain.
    pub scale: u32,
}

impl DisplayPlan {
    /// Phones and tablets (device pixel ratio ≥ 1.5) get a HiDPI display of
    /// half their pixels in points, so text is the size it would be on a
    /// Retina screen; plain monitors get their pixels 1:1.
    pub fn for_screen(css_w: u32, css_h: u32, dpr: f64) -> DisplayPlan {
        let dpr = if dpr.is_finite() && dpr > 0.0 {
            dpr
        } else {
            1.0
        };
        let px_w = ((css_w as f64 * dpr).round() as u32).clamp(640, 7680);
        let px_h = ((css_h as f64 * dpr).round() as u32).clamp(480, 4320);
        let scale = if dpr >= 1.5 { 2 } else { 1 };
        DisplayPlan {
            points_w: (px_w / scale) & !1,
            points_h: (px_h / scale) & !1,
            scale,
        }
    }
}

#[cfg(target_os = "macos")]
async fn extend_display(
    app: &AppHandle,
    name: String,
    serial: u32,
    plan: DisplayPlan,
) -> Result<u32, String> {
    use crate::vdisplay;
    if !vdisplay::available() {
        return Err("this Mac can't make an extra display".into());
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || vdisplay::create(&app, &name, serial, plan))
        .await
        .map_err(|e| format!("cannot make the extra display: {e}"))?
}

#[cfg(not(target_os = "macos"))]
async fn extend_display(
    _app: &AppHandle,
    _name: String,
    _serial: u32,
    _plan: DisplayPlan,
) -> Result<u32, String> {
    // Windows (bundled display driver) and Linux come next.
    Err("extending isn't available on this computer yet".into())
}

#[cfg(target_os = "macos")]
fn remove_display(app: &AppHandle, id: u32) {
    crate::vdisplay::remove(app, id);
}

#[cfg(not(target_os = "macos"))]
fn remove_display(_app: &AppHandle, _id: u32) {}

#[cfg(test)]
mod tests {
    use super::{serial_for, DisplayPlan};

    #[test]
    fn plans_for_phones_and_monitors() {
        // Android tablet 840×614 css at dpr 2.6 → HiDPI, half its pixels.
        let p = DisplayPlan::for_screen(840, 614, 2.6);
        assert_eq!((p.scale, p.points_w, p.points_h), (2, 1092, 798));
        // A 1080p laptop as receiver → 1:1.
        let p = DisplayPlan::for_screen(1920, 1080, 1.0);
        assert_eq!((p.scale, p.points_w, p.points_h), (1, 1920, 1080));
        // Nonsense ratio falls back to 1×.
        assert_eq!(DisplayPlan::for_screen(1280, 800, f64::NAN).scale, 1);
    }

    #[test]
    fn serials_are_stable_and_distinct() {
        assert_eq!(serial_for("a1b2"), serial_for("a1b2"));
        assert_ne!(serial_for("a1b2"), serial_for("a1b3"));
        assert_ne!(serial_for(""), 0);
    }
}
