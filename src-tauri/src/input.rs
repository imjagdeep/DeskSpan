//! Control back from a receiver: its touches, pen strokes, mouse and scroll
//! (the "input" data channel) move and click the real pointer on the screen
//! it shows. Only for devices the user allowed to control the computer, and
//! checked here, not just on the receiver.
//!
//! Touch and pen act as the left mouse button (one finger at a time);
//! a mouse sends its own buttons. Windows: SendInput. macOS: CGEvent, which
//! needs DeskSpan allowed under Privacy & Security › Accessibility. Linux:
//! not yet.

use crate::server::Hub;
use serde::Deserialize;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One event from the receiver (receiver/src/protocol.ts `InputEvent`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "k")]
pub enum Event {
    /// Pointer: e = d(own) m(ove) u(p) c(ancel); x/y 0..1 of the picture.
    #[serde(rename = "p")]
    Pointer {
        e: String,
        id: i64,
        pt: String,
        x: f64,
        y: f64,
        /// Mouse buttons held (1 left, 2 right, 4 middle).
        #[serde(default)]
        b: u32,
    },
    /// Wheel or two-finger scroll, CSS pixels.
    #[serde(rename = "w")]
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    #[serde(other)]
    Other,
}

/// A screen's place on the desktop, in the units the OS moves the pointer in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// 0..1 of the picture → a point on the desktop (kept on the screen).
    pub fn point(&self, fx: f64, fy: f64) -> (f64, f64) {
        let fx = fx.clamp(0.0, 1.0);
        let fy = fy.clamp(0.0, 1.0);
        (
            self.x + fx * (self.w - 1.0).max(0.0),
            self.y + fy * (self.h - 1.0).max(0.0),
        )
    }
}

/// What the pointer should do, in desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Move { x: f64, y: f64 },
    Down { x: f64, y: f64, button: Button },
    Up { x: f64, y: f64, button: Button },
    Scroll { x: f64, y: f64, dx: f64, dy: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

const BUTTONS: [(u32, Button); 3] = [(1, Button::Left), (2, Button::Right), (4, Button::Middle)];

/// Turns receiver events into pointer actions: tracks which finger drives
/// the pointer and which buttons are held, so every down gets its up.
#[derive(Default)]
pub struct Translator {
    /// The pointer (finger/pen) currently driving, if any.
    active: Option<i64>,
    /// Mouse buttons held, as the receiver's bitmask.
    held: u32,
}

impl Translator {
    pub fn translate(&mut self, ev: &Event, rect: Rect) -> Vec<Action> {
        match ev {
            Event::Pointer { e, id, pt, x, y, b } => {
                let (x, y) = rect.point(*x, *y);
                if pt == "m" {
                    self.mouse(e, *b, x, y)
                } else {
                    self.touch(e, *id, x, y)
                }
            }
            Event::Wheel { x, y, dx, dy } => {
                let (x, y) = rect.point(*x, *y);
                vec![Action::Scroll {
                    x,
                    y,
                    dx: *dx,
                    dy: *dy,
                }]
            }
            Event::Other => Vec::new(),
        }
    }

    /// A finger or pen: the first one down is the left button; others are ignored.
    fn touch(&mut self, e: &str, id: i64, x: f64, y: f64) -> Vec<Action> {
        match e {
            "d" if self.active.is_none() => {
                self.active = Some(id);
                vec![
                    Action::Move { x, y },
                    Action::Down {
                        x,
                        y,
                        button: Button::Left,
                    },
                ]
            }
            "m" if self.active == Some(id) => vec![Action::Move { x, y }],
            "u" | "c" if self.active == Some(id) => {
                self.active = None;
                vec![Action::Up {
                    x,
                    y,
                    button: Button::Left,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// A mouse: move, then press/release whatever changed in `b`.
    fn mouse(&mut self, e: &str, b: u32, x: f64, y: f64) -> Vec<Action> {
        // "up"/"cancel" may still list buttons on some browsers; trust the event.
        let now = if e == "c" { 0 } else { b };
        let mut out = vec![Action::Move { x, y }];
        for (bit, button) in BUTTONS {
            let was = self.held & bit != 0;
            let is = now & bit != 0;
            if is && !was {
                out.push(Action::Down { x, y, button });
            } else if was && !is {
                out.push(Action::Up { x, y, button });
            }
        }
        self.held = now;
        out
    }

    /// Release anything still held (the connection ended mid-drag).
    pub fn release(&mut self, at: (f64, f64)) -> Vec<Action> {
        let (x, y) = at;
        // A finger or pen holds the left button too.
        let mut held = self.held;
        if self.active.take().is_some() {
            held |= 1;
        }
        self.held = 0;
        BUTTONS
            .iter()
            .filter(|(bit, _)| held & bit != 0)
            .map(|&(_, button)| Action::Up { x, y, button })
            .collect()
    }
}

/// One receiver's control: checks permission, turns events into actions and
/// hands them to a thread that injects them (system calls, kept off the
/// connection's task).
pub struct Control {
    hub: Arc<Hub>,
    device_id: String,
    /// The extra display it shows, or None for the main screen.
    target: Option<u32>,
    allowed: (bool, Instant),
    rect: Option<(Rect, Instant)>,
    translator: Translator,
    last: (f64, f64),
    tx: Option<mpsc::Sender<Action>>,
}

const RECHECK: Duration = Duration::from_secs(1);

impl Control {
    pub fn new(hub: Arc<Hub>, device_id: String, target: Option<u32>) -> Self {
        Control {
            hub,
            device_id,
            target,
            allowed: (false, Instant::now() - RECHECK * 2),
            rect: None,
            translator: Translator::default(),
            last: (0.0, 0.0),
            tx: None,
        }
    }

    fn allowed(&mut self) -> bool {
        if self.allowed.1.elapsed() >= RECHECK {
            let ok = self
                .hub
                .devices()
                .into_iter()
                .any(|d| d.id == self.device_id && d.control);
            self.allowed = (ok, Instant::now());
        }
        self.allowed.0
    }

    fn rect(&mut self) -> Option<Rect> {
        if let Some((r, at)) = self.rect {
            if at.elapsed() < Duration::from_secs(2) {
                return Some(r);
            }
        }
        match screen_rect(self.target) {
            Ok(r) => {
                self.rect = Some((r, Instant::now()));
                Some(r)
            }
            Err(e) => {
                tracing::warn!("control: {e}");
                self.rect.map(|(r, _)| r)
            }
        }
    }

    fn injector(&mut self) -> &mpsc::Sender<Action> {
        self.tx.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Action>();
            let app = self.hub.app.clone();
            std::thread::spawn(move || {
                let mut sys = System::new(&app);
                while let Ok(a) = rx.recv() {
                    sys.apply(a);
                }
            });
            tx
        })
    }

    /// One message from the data channel.
    pub fn handle(&mut self, data: &[u8]) {
        if !self.allowed() {
            return;
        }
        let ev = match serde_json::from_slice::<Event>(data) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("control: unknown input: {e}");
                return;
            }
        };
        let Some(rect) = self.rect() else { return };
        for a in self.translator.translate(&ev, rect) {
            if let Action::Move { x, y } | Action::Down { x, y, .. } | Action::Up { x, y, .. } = a {
                self.last = (x, y);
            }
            let _ = self.injector().send(a);
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let left = self.translator.release(self.last);
        if let Some(tx) = &self.tx {
            for a in left {
                let _ = tx.send(a);
            }
        }
    }
}

fn screen_rect(target: Option<u32>) -> Result<Rect, String> {
    let all = xcap::Monitor::all().map_err(|e| format!("cannot list the screens: {e}"))?;
    let m = match target {
        Some(id) => all.iter().find(|m| m.id().ok() == Some(id)),
        None => all
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .or(all.first()),
    }
    .ok_or("the screen being shown is gone")?;
    let num = |r: Result<i64, xcap::XCapError>| r.map_err(|e| e.to_string());
    Ok(Rect {
        x: num(m.x().map(i64::from))? as f64,
        y: num(m.y().map(i64::from))? as f64,
        w: num(m.width().map(i64::from))? as f64,
        h: num(m.height().map(i64::from))? as f64,
    })
}

// ---------- the operating system's side ----------

#[cfg(windows)]
struct System;

#[cfg(windows)]
impl System {
    fn new(_app: &tauri::AppHandle) -> Self {
        System
    }

    fn apply(&mut self, a: Action) {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        use windows::Win32::UI::WindowsAndMessaging::*;
        // Absolute coordinates are 0..65535 across the whole virtual desktop.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN) as f64,
                GetSystemMetrics(SM_YVIRTUALSCREEN) as f64,
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2) as f64,
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2) as f64,
            )
        };
        let abs = |x: f64, y: f64| {
            (
                (((x - vx) * 65535.0) / (vw - 1.0)).round() as i32,
                (((y - vy) * 65535.0) / (vh - 1.0)).round() as i32,
            )
        };
        let mouse = |x: f64, y: f64, flags: MOUSE_EVENT_FLAGS, data: i32| {
            let (dx, dy) = abs(x, y);
            INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx,
                        dy,
                        mouseData: data as u32,
                        dwFlags: flags
                            | MOUSEEVENTF_MOVE
                            | MOUSEEVENTF_ABSOLUTE
                            | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        };
        let input = match a {
            Action::Move { x, y } => mouse(x, y, MOUSE_EVENT_FLAGS(0), 0),
            Action::Down { x, y, button } => mouse(
                x,
                y,
                match button {
                    Button::Left => MOUSEEVENTF_LEFTDOWN,
                    Button::Right => MOUSEEVENTF_RIGHTDOWN,
                    Button::Middle => MOUSEEVENTF_MIDDLEDOWN,
                },
                0,
            ),
            Action::Up { x, y, button } => mouse(
                x,
                y,
                match button {
                    Button::Left => MOUSEEVENTF_LEFTUP,
                    Button::Right => MOUSEEVENTF_RIGHTUP,
                    Button::Middle => MOUSEEVENTF_MIDDLEUP,
                },
                0,
            ),
            Action::Scroll { x, y, dx, dy } => {
                // Wheel: 120 per notch (~100 px); finer values scroll smoothly.
                let mut v = Vec::new();
                if dy.abs() >= 1.0 {
                    v.push(mouse(x, y, MOUSEEVENTF_WHEEL, (-dy * 1.2).round() as i32));
                }
                if dx.abs() >= 1.0 {
                    v.push(mouse(x, y, MOUSEEVENTF_HWHEEL, (dx * 1.2).round() as i32));
                }
                unsafe { SendInput(&v, std::mem::size_of::<INPUT>() as i32) };
                return;
            }
        };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    }
}

#[cfg(target_os = "macos")]
struct System {
    held: Option<Button>,
    told: bool,
    app: tauri::AppHandle,
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[cfg(target_os = "macos")]
impl System {
    fn new(app: &tauri::AppHandle) -> Self {
        System {
            held: None,
            told: false,
            app: app.clone(),
        }
    }

    /// macOS drops synthetic events from apps not allowed under
    /// Accessibility: say so once, and open that settings page.
    fn check_trusted(&mut self) -> bool {
        if unsafe { AXIsProcessTrusted() } != 0 {
            return true;
        }
        if !self.told {
            self.told = true;
            crate::notify(
                &self.app,
                "Allow DeskSpan to control the computer",
                "Turn DeskSpan on under Privacy & Security › Accessibility, then try again.",
            );
            let _ = std::process::Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .spawn();
        }
        false
    }

    fn apply(&mut self, a: Action) {
        use objc2_core_foundation::CGPoint;
        use objc2_core_graphics::{
            CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, CGScrollEventUnit,
        };
        if !self.check_trusted() {
            return;
        }
        let cg = |b: Button| match b {
            Button::Left => CGMouseButton::Left,
            Button::Right => CGMouseButton::Right,
            Button::Middle => CGMouseButton::Center,
        };
        let post = |kind: CGEventType, x: f64, y: f64, b: CGMouseButton| {
            let ev = CGEvent::new_mouse_event(None, kind, CGPoint { x, y }, b);
            CGEvent::post(CGEventTapLocation::HIDEventTap, ev.as_deref());
        };
        match a {
            Action::Move { x, y } => {
                // While a button is held, macOS expects "dragged" events.
                let kind = match self.held {
                    Some(Button::Left) => CGEventType::LeftMouseDragged,
                    Some(Button::Right) => CGEventType::RightMouseDragged,
                    Some(Button::Middle) => CGEventType::OtherMouseDragged,
                    None => CGEventType::MouseMoved,
                };
                post(kind, x, y, cg(self.held.unwrap_or(Button::Left)));
            }
            Action::Down { x, y, button } => {
                self.held = Some(button);
                let kind = match button {
                    Button::Left => CGEventType::LeftMouseDown,
                    Button::Right => CGEventType::RightMouseDown,
                    Button::Middle => CGEventType::OtherMouseDown,
                };
                post(kind, x, y, cg(button));
            }
            Action::Up { x, y, button } => {
                if self.held == Some(button) {
                    self.held = None;
                }
                let kind = match button {
                    Button::Left => CGEventType::LeftMouseUp,
                    Button::Right => CGEventType::RightMouseUp,
                    Button::Middle => CGEventType::OtherMouseUp,
                };
                post(kind, x, y, cg(button));
            }
            Action::Scroll { dx, dy, .. } => {
                let ev = CGEvent::new_scroll_wheel_event2(
                    None,
                    CGScrollEventUnit::Pixel,
                    2,
                    (-dy).round() as i32,
                    (-dx).round() as i32,
                    0,
                );
                CGEvent::post(CGEventTapLocation::HIDEventTap, ev.as_deref());
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct System {
    told: bool,
    app: tauri::AppHandle,
}

#[cfg(not(any(windows, target_os = "macos")))]
impl System {
    fn new(app: &tauri::AppHandle) -> Self {
        System {
            told: false,
            app: app.clone(),
        }
    }

    fn apply(&mut self, _a: Action) {
        if !self.told {
            self.told = true;
            crate::notify(
                &self.app,
                "Control from devices isn't available on Linux yet",
                "The picture works; touch and mouse from the device come in a later version.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        x: 1920.0,
        y: 0.0,
        w: 1281.0,
        h: 801.0,
    };

    fn ev(json: &str) -> Event {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_the_receivers_messages() {
        assert_eq!(
            ev(
                r#"{"k":"p","e":"d","id":3,"pt":"t","x":0.5,"y":0.25,"pr":0.5,"tx":0,"ty":0,"b":1}"#
            ),
            Event::Pointer {
                e: "d".into(),
                id: 3,
                pt: "t".into(),
                x: 0.5,
                y: 0.25,
                b: 1
            }
        );
        assert!(matches!(
            ev(r#"{"k":"w","x":0,"y":0,"dx":0,"dy":-40}"#),
            Event::Wheel { .. }
        ));
        assert_eq!(ev(r#"{"k":"key","code":"KeyA"}"#), Event::Other);
    }

    #[test]
    fn points_land_on_the_shown_screen() {
        assert_eq!(R.point(0.0, 0.0), (1920.0, 0.0));
        assert_eq!(R.point(1.0, 1.0), (3200.0, 800.0));
        assert_eq!(R.point(0.5, 0.5), (2560.0, 400.0));
        // Off the picture: kept on the screen's edge.
        assert_eq!(R.point(-0.2, 1.7), (1920.0, 800.0));
    }

    #[test]
    fn a_tap_is_down_then_up_and_other_fingers_wait() {
        let mut t = Translator::default();
        let down = t.translate(&ev(r#"{"k":"p","e":"d","id":1,"pt":"t","x":0,"y":0}"#), R);
        assert_eq!(down.len(), 2);
        assert!(matches!(
            down[1],
            Action::Down {
                button: Button::Left,
                ..
            }
        ));
        // A second finger meanwhile does nothing.
        assert!(t
            .translate(
                &ev(r#"{"k":"p","e":"d","id":2,"pt":"t","x":0.5,"y":0.5}"#),
                R
            )
            .is_empty());
        assert!(t
            .translate(
                &ev(r#"{"k":"p","e":"m","id":2,"pt":"t","x":0.6,"y":0.5}"#),
                R
            )
            .is_empty());
        assert_eq!(
            t.translate(&ev(r#"{"k":"p","e":"m","id":1,"pt":"t","x":0.1,"y":0}"#), R)
                .len(),
            1
        );
        let up = t.translate(&ev(r#"{"k":"p","e":"u","id":1,"pt":"t","x":0.1,"y":0}"#), R);
        assert!(matches!(
            up[0],
            Action::Up {
                button: Button::Left,
                ..
            }
        ));
        // Now the next finger can drive.
        assert_eq!(
            t.translate(
                &ev(r#"{"k":"p","e":"d","id":2,"pt":"t","x":0.5,"y":0.5}"#),
                R
            )
            .len(),
            2
        );
    }

    #[test]
    fn mouse_buttons_press_and_release_by_change() {
        let mut t = Translator::default();
        assert_eq!(
            t.translate(
                &ev(r#"{"k":"p","e":"m","id":1,"pt":"m","x":0,"y":0,"b":0}"#),
                R
            )
            .len(),
            1
        );
        let right = t.translate(
            &ev(r#"{"k":"p","e":"d","id":1,"pt":"m","x":0,"y":0,"b":2}"#),
            R,
        );
        assert!(matches!(
            right[1],
            Action::Down {
                button: Button::Right,
                ..
            }
        ));
        let up = t.translate(
            &ev(r#"{"k":"p","e":"u","id":1,"pt":"m","x":0,"y":0,"b":0}"#),
            R,
        );
        assert!(matches!(
            up[1],
            Action::Up {
                button: Button::Right,
                ..
            }
        ));
    }

    #[test]
    fn a_drag_cut_short_is_released() {
        let mut t = Translator::default();
        t.translate(&ev(r#"{"k":"p","e":"d","id":1,"pt":"p","x":0,"y":0}"#), R);
        let left = t.release((5.0, 5.0));
        assert_eq!(
            left,
            vec![Action::Up {
                x: 5.0,
                y: 5.0,
                button: Button::Left
            }]
        );
        assert!(t.release((0.0, 0.0)).is_empty());
    }
}
