//! DeskSpan Screen: the receiver page in a native app. All the work happens
//! in the page (WebRTC, touch, files); the app adds a QR scanner, full
//! screen without browser bars, and a home-screen icon.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_barcode_scanner::init())
        .run(tauri::generate_context!())
        .expect("error while running DeskSpan Screen");
}
