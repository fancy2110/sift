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
use disks::list_volumes;
use scanner::{cancel_scan, scan_running, set_scan_focus, start_scan, ScanManager};
use watcher::{unwatch_fs, watch_fs, FsWatcherState};
use tauri::Manager as _;

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn diag_log(app: tauri::AppHandle, message: String) -> Result<(), String> {
    use std::io::Write;
    // 1. Mirror into the native window title so it is readable externally
    //    via AXTitle without any webview access.
    if let Some(w) = app.get_webview_window("main") {
        let short: String = message.chars().take(140).collect();
        let title = format!("DIAG {short}");
        let _ = w.set_title(&title);
    }
    // 2. Persist to a file in the home directory as well.
    if let Some(home) = home_dir() {
        let path = home.join("sift-diag.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{} {message}", chrono_like_ts());
        }
    }
    Ok(())
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn chrono_like_ts() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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
            diag_log,
            // volumes and scanning
            list_volumes,
            start_scan,
            cancel_scan,
            set_scan_focus,
            scan_running,
            // deletion and filesystem awareness
            move_to_trash,
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
