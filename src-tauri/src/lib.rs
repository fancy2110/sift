//! Sift's Tauri front end.
//!
//! Every module in this crate is an adapter. The scanning, the accounting, the
//! analysis, the local state and the background monitor all live in the shared
//! `sift-*` crates. Nothing here contains product logic; it only registers
//! commands and translates event vocabularies.

mod analyze;
mod cleanup;
mod credentials;
mod disks;
mod scanner;
mod schedule;
mod tray;
mod watcher;

use analyze::AppServices;
use disks::list_volumes;
use scanner::{
    cancel_scan, list_dir_files, pause_scan, resolve_permission, resume_scan, scan_running,
    set_scan_focus, start_scan, ScanManager,
};
use tauri::Manager as _;
use watcher::{unwatch_fs, watch_fs, FsWatcherState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .runtime(tauri_runtime_wry::Wry::default())
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
            // volumes and scanning
            list_volumes,
            start_scan,
            cancel_scan,
            pause_scan,
            resume_scan,
            set_scan_focus,
            scan_running,
            resolve_permission,
            list_dir_files,
            // deletion and filesystem awareness
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
            analyze::set_auto_clean_mode,
            analyze::set_scheduled_cleanup,
            analyze::mark_path,
            analyze::take_store_warnings,
            analyze::store_location,
            analyze::routine_suggestions,
            analyze::list_history,
            analyze::list_routines,
            analyze::accept_routine_suggestion,
            analyze::save_routine,
            analyze::dismiss_routine_suggestion,
            analyze::delete_routine,
            analyze::toggle_routine_mode,
            analyze::run_routine,
            analyze::get_language,
            analyze::set_language,
            analyze::get_ai_config,
            analyze::save_ai_config,
        ])
        .setup(|app| {
            tray::build(app.handle())?;
            schedule::start(app.handle());

            // Closing the window keeps the app in the tray; the tray menu's
            // quit is the real exit.
            let handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Sift")
        .run(|app, event| {
            // Flush conclusions and stop the watcher on the way out.
            if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
                analyze::shutdown(app);
            }
        });
}
