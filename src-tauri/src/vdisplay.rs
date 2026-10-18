//! macOS: a virtual display per "extend" receiver, so the phone or tablet
//! becomes a real extra monitor (System Settings › Displays lists it, windows
//! can be dragged onto it).
//!
//! Uses CoreGraphics' CGVirtualDisplay classes. They are not in Apple's
//! public headers but have shipped since macOS 11 and are what DeskPad and
//! BetterDisplay use; DeskSpan is distributed outside the App Store. If a
//! macOS update removes them, `available()` turns false and receivers are
//! mirrored instead.
//!
//! AppKit/CoreGraphics objects here live on the main thread only: they are
//! created and released there (`run_on_main_thread`) and kept in a
//! main-thread map keyed by display ID.

use crate::media::DisplayPlan;
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSSize, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;

thread_local! {
    /// Main thread only: the live displays (dropping one removes it).
    static DISPLAYS: RefCell<HashMap<u32, Retained<AnyObject>>> = RefCell::new(HashMap::new());
}

/// True when this Mac has the virtual display classes.
pub fn available() -> bool {
    AnyClass::get(c"CGVirtualDisplay").is_some()
        && AnyClass::get(c"CGVirtualDisplayDescriptor").is_some()
}

/// Create a display (from any thread; the work happens on the main thread).
/// Returns its CGDirectDisplayID.
pub fn create(app: &AppHandle, name: &str, serial: u32, plan: DisplayPlan) -> Result<u32, String> {
    let (tx, rx) = mpsc::channel();
    let name = name.to_string();
    app.run_on_main_thread(move || {
        let _ = tx.send(unsafe { create_here(&name, serial, plan) });
    })
    .map_err(|e| format!("cannot reach the main thread: {e}"))?;
    let id = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "macOS didn't create the extra display in time".to_string())??;
    // Give WindowServer a moment to add it before capture starts.
    std::thread::sleep(Duration::from_millis(600));
    Ok(id)
}

/// The descriptor's pixel ceiling per side (8K).
const MAX_PIXELS: u32 = 8192;

/// Give a display a new size (the receiver went full screen or rotated).
pub fn resize(app: &AppHandle, id: u32, plan: DisplayPlan) {
    let done = app.run_on_main_thread(move || {
        let found = DISPLAYS.with(|d| d.borrow().get(&id).cloned());
        let Some(display) = found else { return };
        match unsafe { apply(&display, plan) } {
            Ok(()) => tracing::info!(
                "virtual display {id} now {}x{} points @{}x",
                plan.points_w,
                plan.points_h,
                plan.scale
            ),
            Err(e) => tracing::warn!("cannot resize virtual display {id}: {e}"),
        }
    });
    if let Err(e) = done {
        tracing::warn!("cannot resize virtual display {id}: {e}");
    }
}

/// Remove a display (windows on it move back to the other screens).
pub fn remove(app: &AppHandle, id: u32) {
    let done = app.run_on_main_thread(move || {
        DISPLAYS.with(|d| d.borrow_mut().remove(&id));
    });
    if let Err(e) = done {
        tracing::warn!("cannot remove virtual display {id}: {e}");
    }
}

/// Main thread only: set the display's one mode (size, HiDPI or not).
unsafe fn apply(display: &AnyObject, plan: DisplayPlan) -> Result<(), String> {
    let missing = || "this Mac can't size the extra display (macOS changed it)".to_string();
    let set_cls = AnyClass::get(c"CGVirtualDisplaySettings").ok_or_else(missing)?;
    let mode_cls = AnyClass::get(c"CGVirtualDisplayMode").ok_or_else(missing)?;
    let settings: Retained<AnyObject> = msg_send![set_cls, new];
    let _: () = msg_send![&*settings, setHiDPI: u32::from(plan.scale == 2)];
    let mode_alloc: Allocated<AnyObject> = msg_send![mode_cls, alloc];
    let mode: Option<Retained<AnyObject>> = msg_send![
        mode_alloc,
        initWithWidth: plan.points_w as usize,
        height: plan.points_h as usize,
        refreshRate: 60.0f64
    ];
    let mode = mode.ok_or("macOS refused the display size")?;
    let modes = NSArray::from_retained_slice(&[mode]);
    let _: () = msg_send![&*settings, setModes: &*modes];
    let ok: bool = msg_send![display, applySettings: &*settings];
    if ok {
        Ok(())
    } else {
        Err(format!(
            "macOS refused a {}×{} display",
            plan.points_w, plan.points_h
        ))
    }
}

/// Main thread only.
unsafe fn create_here(name: &str, serial: u32, plan: DisplayPlan) -> Result<u32, String> {
    let missing = || "this Mac can't make an extra display (macOS changed it)".to_string();
    let desc_cls = AnyClass::get(c"CGVirtualDisplayDescriptor").ok_or_else(missing)?;
    let disp_cls = AnyClass::get(c"CGVirtualDisplay").ok_or_else(missing)?;

    let px_w = plan.points_w * plan.scale;
    let px_h = plan.points_h * plan.scale;
    let desc: Retained<AnyObject> = msg_send![desc_cls, new];
    let queue =
        dispatch2::DispatchQueue::main() as *const dispatch2::DispatchQueue as *const AnyObject;
    let _: () = msg_send![&*desc, setDispatchQueue: queue];
    let ns_name = NSString::from_str(name);
    let _: () = msg_send![&*desc, setName: &*ns_name];
    // The largest it may ever become: room for both orientations and for
    // full screen or a bigger device, so `resize` can reuse the display
    // (the descriptor's ceiling can't change later).
    let max = px_w.max(px_h).max(MAX_PIXELS);
    let _: () = msg_send![&*desc, setMaxPixelsWide: max];
    let _: () = msg_send![&*desc, setMaxPixelsHigh: max];
    // A plausible physical size (about 110 points per inch).
    let mm = NSSize::new(
        plan.points_w as f64 * 25.4 / 110.0,
        plan.points_h as f64 * 25.4 / 110.0,
    );
    let _: () = msg_send![&*desc, setSizeInMillimeters: mm];
    // macOS remembers each display's arrangement by vendor/product/serial.
    let _: () = msg_send![&*desc, setVendorID: 0x4453u32]; // "DS"
    let _: () = msg_send![&*desc, setProductID: 0x5350u32]; // "SP"
    let _: () = msg_send![&*desc, setSerialNum: serial];

    let alloc: Allocated<AnyObject> = msg_send![disp_cls, alloc];
    let display: Option<Retained<AnyObject>> = msg_send![alloc, initWithDescriptor: &*desc];
    let display = display.ok_or("macOS refused to create the extra display")?;

    apply(&display, plan)?;
    let id: u32 = msg_send![&*display, displayID];
    if id == 0 {
        return Err("macOS gave the extra display no ID".into());
    }
    tracing::info!(
        "virtual display {id}: {}x{} points @{}x",
        plan.points_w,
        plan.points_h,
        plan.scale
    );
    DISPLAYS.with(|d| d.borrow_mut().insert(id, display));
    Ok(id)
}
