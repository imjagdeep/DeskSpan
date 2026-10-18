// Desktop entry only for `tauri dev` checks; the app ships on iOS/Android.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    deskspan_screen_lib::run()
}
