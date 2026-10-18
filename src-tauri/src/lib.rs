//! DeskSpan: use a phone, tablet or another computer as a second screen.
//! Tray app with a small local server (server.rs) that receivers open in a
//! browser; media.rs gives each receiver a WebRTC connection (peer.rs) with
//! the encoded screen (video.rs, Cisco's OpenH264 via h264lib.rs).

mod account;
mod desklab;
mod devices;
mod h264lib;
mod media;
mod peer;
#[cfg(target_os = "macos")]
mod sck;
mod secret;
mod server;
mod settings;
mod tray;
mod video;

use server::{Decision, Hub};
use settings::Settings;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

pub struct AppState {
    data_dir: PathBuf,
    settings: Mutex<Settings>,
}

impl AppState {
    /// A panic elsewhere must not lock everyone out: take the data anyway.
    fn settings(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A small system notification; failures only matter to the log.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notification failed: {e}");
    }
}

/// Start the server with the saved port and network choice.
pub async fn start_sharing_now(app: &AppHandle) -> CmdResult<()> {
    let (port, everywhere) = {
        let st = app.state::<AppState>();
        let s = st.settings();
        (s.port, s.listen_everywhere)
    };
    let hub = app.state::<Arc<Hub>>().inner().clone();
    hub.start(port, everywhere).await
}

pub fn stop_sharing_now(app: &AppHandle) {
    app.state::<Arc<Hub>>().stop();
}

#[derive(serde::Serialize)]
struct AppInfo {
    version: String,
    platform: &'static str,
    /// This computer's name, as receivers see it.
    host_name: String,
}

#[tauri::command]
fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS,
        host_name: gethostname::gethostname().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings().clone()
}

/// Saved; if the port or network choice changed while sharing, sharing
/// restarts with the new ones.
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    hub: State<'_, Arc<Hub>>,
    settings: Settings,
) -> CmdResult<Settings> {
    let mut next = settings.clamp();
    let before = state.settings().clone();
    // The app decides this one, not the page.
    next.autostart_applied = before.autostart_applied;
    next.save(&state.data_dir)?;
    *state.settings() = next.clone();
    let moved = next.port != before.port || next.listen_everywhere != before.listen_everywhere;
    if moved && hub.status().running {
        hub.stop();
        start_sharing_now(&app).await?;
    }
    Ok(next)
}

#[tauri::command]
async fn start_sharing(app: AppHandle) -> CmdResult<()> {
    start_sharing_now(&app).await
}

#[tauri::command]
fn stop_sharing(hub: State<'_, Arc<Hub>>) {
    hub.stop();
}

#[tauri::command]
fn hub_status(hub: State<'_, Arc<Hub>>) -> server::Status {
    hub.status()
}

/// The picture: preparing / streaming / error, size and frame rate.
#[tauri::command]
fn video_status(video: State<'_, video::Video>) -> video::VideoStatus {
    video.status()
}

/// A fresh QR link and code; the old ones stop working.
#[tauri::command]
fn new_pairing(hub: State<'_, Arc<Hub>>) -> CmdResult<()> {
    hub.new_pairing()
}

/// The answer to "Allow <device>?".
#[tauri::command]
fn answer_request(
    hub: State<'_, Arc<Hub>>,
    id: String,
    allow: bool,
    control: bool,
) -> CmdResult<()> {
    hub.answer(&id, Decision { allow, control })
}

#[tauri::command]
fn devices_list(hub: State<'_, Arc<Hub>>) -> Vec<devices::Device> {
    hub.devices()
}

#[tauri::command]
fn device_remove(hub: State<'_, Arc<Hub>>, id: String) -> CmdResult<()> {
    hub.remove_device(&id)
}

#[tauri::command]
fn device_set_control(hub: State<'_, Arc<Hub>>, id: String, control: bool) -> CmdResult<()> {
    hub.set_control(&id, control)
}

#[tauri::command]
fn device_rename(hub: State<'_, Arc<Hub>>, id: String, name: String) -> CmdResult<()> {
    hub.rename_device(&id, &name)
}

/// Only these pages can be opened from the app.
#[tauri::command]
fn open_link(app: AppHandle, id: String) -> CmdResult<()> {
    let url = match id.as_str() {
        "site" => "https://getdesklab.com/",
        "repo" => "https://github.com/imjagdeep/DeskSpan",
        "issues" => "https://github.com/imjagdeep/DeskSpan/issues",
        "releases" => "https://github.com/imjagdeep/DeskSpan/releases/latest",
        "developer" => "https://github.com/imjagdeep",
        "deskzero" => "https://github.com/imjagdeep/DeskZero/releases/latest",
        "deskmedic" => "https://github.com/imjagdeep/DeskMedic/releases/latest",
        "deskfolio" => "https://github.com/imjagdeep/DeskFolio/releases/latest",
        "deskshot" => "https://github.com/imjagdeep/DeskShot/releases/latest",
        // macOS: System Settings › Privacy & Security › Screen Recording.
        "screen-permission" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        _ => return Err(format!("unknown link {id}")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// The other DeskLab tools and whether each is installed here.
#[tauri::command]
fn desklab_tools() -> Vec<desklab::LabTool> {
    desklab::list("deskspan")
}

/// Start another DeskLab tool, optionally handing it one file.
#[tauri::command]
fn open_tool(id: String, file: Option<PathBuf>) -> CmdResult<()> {
    desklab::open(&id, file.as_deref())
}

// ---------- the optional DeskLab account (all calls go through account.rs) ----------

type Acct<'a> = State<'a, account::Account>;

#[tauri::command]
async fn account_status(acct: Acct<'_>) -> CmdResult<account::Status> {
    acct.status().await.map_err(account::plain)
}

#[tauri::command]
async fn account_send_code(acct: Acct<'_>, email: String) -> CmdResult<()> {
    acct.send_code(&email).await.map_err(account::plain)
}

#[tauri::command]
async fn account_verify(acct: Acct<'_>, email: String, code: String) -> CmdResult<()> {
    acct.verify(&email, &code).await.map_err(account::plain)
}

#[tauri::command]
async fn account_login(acct: Acct<'_>, email: String, password: String) -> CmdResult<()> {
    acct.login(&email, &password).await.map_err(account::plain)
}

#[tauri::command]
async fn account_logout(acct: Acct<'_>) -> CmdResult<()> {
    acct.logout().await.map_err(account::plain)
}

#[tauri::command]
async fn account_set_password(
    acct: Acct<'_>,
    password: String,
    current: Option<String>,
) -> CmdResult<()> {
    acct.set_password(&password, current.as_deref())
        .await
        .map_err(account::plain)
}

#[tauri::command]
async fn account_remove_password(acct: Acct<'_>) -> CmdResult<()> {
    acct.remove_password().await.map_err(account::plain)
}

#[tauri::command]
async fn account_sign_out_device(acct: Acct<'_>, id: String) -> CmdResult<()> {
    acct.sign_out_device(&id).await.map_err(account::plain)
}

#[tauri::command]
async fn account_delete(acct: Acct<'_>, confirm: String) -> CmdResult<()> {
    acct.delete(&confirm).await.map_err(account::plain)
}

/// A downloaded update is ready: tell someone who only has the tray icon.
#[tauri::command]
fn notify_update(app: AppHandle, version: String) {
    notify(
        &app,
        &format!("DeskSpan {version} is ready"),
        "Open DeskSpan and choose Restart to update. It takes a few seconds.",
    );
}

/// Installed copies start with the computer (tray ready, trusted devices
/// can reconnect); offered once, then it's the user's setting.
fn first_start_autostart(app: &AppHandle, s: &mut Settings) {
    use tauri_plugin_autostart::ManagerExt;
    if s.autostart_applied {
        return;
    }
    s.autostart_applied = true;
    let installed = std::env::current_exe()
        .map(|p| {
            let p = p.to_string_lossy().to_lowercase();
            p.contains("appdata") || p.starts_with("/applications")
        })
        .unwrap_or(false);
    if installed {
        if let Err(e) = app.autolaunch().enable() {
            tracing::warn!("could not turn on start with the computer: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Ignored if a subscriber is already set (tests).
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .try_init();
    tauri::Builder::default()
        // Must be first: a second launch opens the running window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // Reopen where it was left, never hidden.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .setup(|app| {
            // DESKSPAN_DATA_DIR points it elsewhere (testing, portable use).
            let data_dir = match std::env::var_os("DESKSPAN_DATA_DIR") {
                Some(d) => PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            let mut settings = Settings::load(&data_dir);
            first_start_autostart(app.handle(), &mut settings);
            if let Err(e) = settings.save(&data_dir) {
                tracing::warn!("{e}");
            }
            let start_now = settings.start_on_launch;
            let hub = Hub::new(app.handle().clone(), &data_dir);
            let video = video::Video::new(data_dir.clone());
            media::start(hub.clone(), video.clone());
            app.manage(hub);
            app.manage(video);
            app.manage(AppState {
                data_dir,
                settings: Mutex::new(settings),
            });
            app.manage(account::Account::new("deskspan")?);
            tray::build(app.handle())?;

            if start_now {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = start_sharing_now(&handle).await {
                        notify(&handle, "DeskSpan could not start sharing", &e);
                    }
                });
            }
            // Started with the computer: stay in the tray.
            if std::env::args().any(|a| a == "--background") {
                tray::set_in_dock(app.handle(), false);
            } else {
                tray::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps DeskSpan (and sharing) in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    tray::set_in_dock(window.app_handle(), false);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_settings,
            save_settings,
            start_sharing,
            stop_sharing,
            hub_status,
            video_status,
            new_pairing,
            answer_request,
            devices_list,
            device_remove,
            device_set_control,
            device_rename,
            open_link,
            desklab_tools,
            open_tool,
            notify_update,
            account_status,
            account_send_code,
            account_verify,
            account_login,
            account_logout,
            account_set_password,
            account_remove_password,
            account_sign_out_device,
            account_delete,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DeskSpan");
}
