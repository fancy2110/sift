//! Sift's Tauri front end.
//!
//! Every module in this crate is an adapter. The scanning, the accounting, the
//! analysis, the local state and the background monitor all live in the shared
//! `sift-*` crates, which the native GPUI application uses too. Nothing here
//! contains product logic that the other front end would have to re-implement;
//! it only registers commands and translates event vocabularies.

mod analyze;
mod cleanup;
mod disks;
mod scanner;
mod watcher;

use analyze::AppServices;
use cleanup::move_to_trash;
use disks::{home_dir, list_volumes};
use scanner::{cancel_scan, scan_running, set_scan_focus, start_scan, ScanManager};
use watcher::{unwatch_fs, watch_fs, FsWatcherState};

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
        .manage(AppServices::new())
        .manage(FsWatcherState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            // volumes and scanning
            list_volumes,
            home_dir,
            start_scan,
            cancel_scan,
            set_scan_focus,
            scan_running,
            // deletion and filesystem awareness
            move_to_trash,
            cleanup::preview_delete,
            watch_fs,
            unwatch_fs,
            // analysis, persistence and monitoring
            analyze::analyze_current,
            analyze::list_cleanable,
            analyze::set_cleanable_approval,
            analyze::approve_structural,
            analyze::forget_cleanable,
            analyze::clean_paths,
            analyze::monitor_status,
            analyze::start_monitor,
            analyze::stop_monitor,
            analyze::take_store_warnings,
            analyze::store_location,
            analyze::routine_suggestions,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sift")
        .run(|app, event| {
            // Flush conclusions and stop the watcher on the way out.
            if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
                analyze::shutdown(app);
            }
        });
}
