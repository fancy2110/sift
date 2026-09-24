mod cleanup;
mod disks;
mod scanner;
mod watcher;

use cleanup::move_to_trash;
use disks::list_volumes;
use scanner::{cancel_scan, set_scan_focus, start_scan, ScanManager};
use watcher::{watch_fs, FsWatcherState};

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(ScanManager::new())
        .manage(FsWatcherState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            list_volumes,
            start_scan,
            cancel_scan,
            set_scan_focus,
            move_to_trash,
            watch_fs
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sift");
}
