//! Tray (Windows, Linux) / menu-bar (macOS) icon: start and stop sharing.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let start = MenuItem::with_id(app, "start", "Start sharing", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop sharing", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open DeskSpan", true, None::<&str>)?;
    let devices = MenuItem::with_id(app, "devices", "Devices", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &start,
            &stop,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &devices,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("DeskSpan")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "start" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::start_sharing_now(&app).await {
                        crate::notify(&app, "DeskSpan could not start sharing", &e);
                    }
                });
            }
            "stop" => crate::stop_sharing_now(app),
            "open" => show_main(app),
            "devices" | "settings" => {
                show_main(app);
                let _ = app.emit("navigate", event.id().as_ref());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    match tray_image() {
        Some(icon) => builder = builder.icon(icon),
        None => {
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
        }
    }
    builder.build(app)?;
    Ok(())
}

/// The small icon drawn for the tray: the 32 px, or 64 px for the macOS
/// menu bar (2x).
fn tray_image() -> Option<tauri::image::Image<'static>> {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = include_bytes!("../icons/64x64.png");
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = include_bytes!("../icons/32x32.png");
    match tauri::image::Image::from_bytes(bytes) {
        Ok(i) => Some(i),
        Err(e) => {
            tracing::warn!("tray icon: {e}");
            None
        }
    }
}

/// macOS: in the Dock only while the window is open; otherwise DeskSpan
/// lives in the menu bar alone.
#[cfg(target_os = "macos")]
pub fn set_in_dock(app: &AppHandle, visible: bool) {
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    if let Err(e) = app.set_activation_policy(policy) {
        tracing::warn!("could not change the Dock icon: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_in_dock(_app: &AppHandle, _visible: bool) {}

pub fn show_main(app: &AppHandle) {
    set_in_dock(app, true);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
