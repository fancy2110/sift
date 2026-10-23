//! Sift's Tauri front end.
//!
//! Every module in this crate is an adapter. The scanning, the accounting, the
//! analysis, the local state and the background monitor all live in the shared
//! `sift-*` crates. Nothing here contains product logic; it only registers
//! commands and translates event vocabularies.

mod analyze;
mod cleanup;
mod cleanup_rules;
mod credentials;
mod disks;
mod scanner;
mod schedule;
pub mod startup_trace;
mod tray;
mod watcher;

use analyze::AppServices;
use disks::{home_dir, list_volumes};
use scanner::{
    cancel_scan, list_dir_files, pause_scan, resolve_permission, resume_scan, scan_running,
    set_scan_focus, start_scan, ScanManager,
};
use tauri::Manager as _;
use watcher::{unwatch_fs, watch_fs, FsWatcherState};

/// Snapshot of one startup trace mark for the `startup_trace` command.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupMarkDto {
    label: String,
    since_origin_ms: u64,
    wall_ms: Option<i64>,
}

/// Read the collected startup phase marks (always available; cheap; no secrets).
#[tauri::command]
fn startup_trace() -> Vec<StartupMarkDto> {
    startup_trace::marks()
        .into_iter()
        .map(|mark| StartupMarkDto {
            label: mark.label,
            since_origin_ms: mark.since_origin_ms,
            wall_ms: mark.wall_ms,
        })
        .collect()
}

/// Let the webview contribute its own marks (module eval, first paint, data
/// ready) aligned to the native wall-clock anchor.
#[tauri::command]
fn startup_mark(label: String, wall_ms: i64) {
    startup_trace::mark_from_webview(&label, wall_ms);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    startup_trace::mark("run:enter");
    let services_timer = std::time::Instant::now();
    let services = AppServices::new();
    startup_trace::mark(&format!(
        "services:new ({} ms)",
        services_timer.elapsed().as_millis()
    ));

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
        .manage(services)
        .manage(FsWatcherState::default())
        .invoke_handler(tauri::generate_handler![
            // startup diagnostics
            startup_trace,
            startup_mark,
            // volumes and scanning
            list_volumes,
            home_dir,
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
            analyze::cancel_cleanup,
            analyze::monitor_status,
            analyze::start_monitor,
            analyze::stop_monitor,
            analyze::set_auto_clean_mode,
            analyze::set_scheduled_cleanup,
            analyze::mark_path,
            analyze::ai_prompt,
            analyze::set_finding_dismissed,
            cleanup_rules::list_cleanup_rules,
            cleanup_rules::save_cleanup_rule,
            cleanup_rules::delete_cleanup_rule,
            cleanup_rules::preview_cleanup_rule,
            cleanup_rules::run_cleanup_rule,
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
            analyze::save_ai_profile,
            analyze::select_ai_profile,
            analyze::delete_ai_profile,
            analyze::set_ai_enabled,
            analyze::set_ai_language,
        ])
        .setup(|app| {
            startup_trace::mark("setup:enter");
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
            startup_trace::mark("setup:done");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Sift")
        .run(|app, event| {
            match event {
                // Clicking the Dock icon while the window is hidden (the app lives in
                // the tray) must bring the window back; otherwise the click appears
                // to do nothing.
                tauri::RunEvent::Reopen { .. } => tray::show_window(app),
              
                // The tray icon and menu are not needed to show the window.
                // Build them on the first event-loop turn after the webview
                // has been created, keeping NSStatusBar/menu construction out
                // of the pre-window critical path.
                tauri::RunEvent::Ready => {
                    startup_trace::mark("run:ready");
                    if let Err(err) = tray::build(app) {
                        eprintln!("tray build failed: {err}");
                    }
                    startup_trace::mark("run:tray-built");
                }
                // Flush conclusions and stop the watcher on the way out.
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                    analyze::shutdown(app);
                }
                _ => {}
            }
        });
}
