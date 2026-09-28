//! System tray: direct interaction without opening the window.
//!
//! Exposes window control, the full scan lifecycle (start, pause, resume,
//! cancel), a one-pass cleanup of remembered safe items, the background
//! monitor switch, the auto-clean switch, login autostart and quit. While a
//! scan runs, a refresh thread keeps the menu and tooltip current.

use std::time::Duration;

use sift_analyze::Safety;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

/// Build the tray icon and its menu. Called once during startup.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    TrayIconBuilder::with_id("sift-tray")
        .icon(app.default_window_icon().cloned().expect("app has an icon"))
        .tooltip(tray_text(app, "tray.tooltip"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            // Left click toggles the main window.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Build the menu with labels reflecting the current runtime state.
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::DynRuntime>> {
    let services = app.state::<crate::analyze::AppServices>();
    let manager = app.state::<crate::scanner::ScanManager>();
    let auto_on = services.store.settings().monitor.auto_mode.may_delete();
    let monitor_on = services.monitor_running();
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);

    let open = MenuItem::with_id(app, "open", tray_text(app, "tray.open"), true, None::<&str>)?;
    let sep_settings = PredefinedMenuItem::separator(app)?;
    let sep_quit = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", tray_text(app, "tray.quit"), true, None::<&str>)?;
    let autostart = MenuItem::with_id(
        app,
        "autostart",
        tray_text(
            app,
            if autostart_on {
                "tray.autostartOn"
            } else {
                "tray.autostartOff"
            },
        ),
        true,
        None::<&str>,
    )?;
    let auto = MenuItem::with_id(
        app,
        "auto",
        tray_text(
            app,
            if auto_on {
                "tray.autoOn"
            } else {
                "tray.autoOff"
            },
        ),
        true,
        None::<&str>,
    )?;
    let monitor = MenuItem::with_id(
        app,
        "monitor",
        tray_text(
            app,
            if monitor_on {
                "tray.monitorOn"
            } else {
                "tray.monitorOff"
            },
        ),
        true,
        None::<&str>,
    )?;

    if manager.is_running() {
        // Scan in progress: pause/resume and cancel replace the start action.
        let pause_or_resume = if manager.is_paused() {
            MenuItem::with_id(app, "resume", tray_text(app, "tray.resume"), true, None::<&str>)?
        } else {
            MenuItem::with_id(app, "pause", tray_text(app, "tray.pause"), true, None::<&str>)?
        };
        let cancel = MenuItem::with_id(
            app,
            "cancel",
            tray_text(app, "tray.cancelScan"),
            true,
            None::<&str>,
        )?;
        let clean = MenuItem::with_id(
            app,
            "clean",
            tray_text(app, "tray.clean"),
            false,
            None::<&str>,
        )?;

        Menu::with_items(
            app,
            &[
                &open,
                &pause_or_resume,
                &cancel,
                &sep_settings,
                &clean,
                &auto,
                &monitor,
                &sep_quit,
                &autostart,
                &quit,
            ],
        )
    } else {
        let scan =
            MenuItem::with_id(app, "scan", tray_text(app, "tray.scan"), true, None::<&str>)?;
        let clean =
            MenuItem::with_id(app, "clean", tray_text(app, "tray.clean"), true, None::<&str>)?;

        Menu::with_items(
            app,
            &[
                &open,
                &scan,
                &sep_settings,
                &clean,
                &auto,
                &monitor,
                &sep_quit,
                &autostart,
                &quit,
            ],
        )
    }
}

/// Render a tray string in the persisted interface language. The tray is a
/// native surface, so its text cannot go through the webview's `t()` helper.
fn tray_text(app: &AppHandle, key: &str) -> std::borrow::Cow<'static, str> {
    let english = app
        .state::<crate::analyze::AppServices>()
        .store
        .settings()
        .language
        == "en";
    match (english, key) {
        (true, "tray.tooltip") => "Sift Disk Space Manager".into(),
        (true, "tray.open") => "Open Sift".into(),
        (true, "tray.scan") => "Scan Now".into(),
        (true, "tray.cancelScan") => "Cancel Scan".into(),
        (true, "tray.pause") => "Pause Scan".into(),
        (true, "tray.resume") => "Resume Scan".into(),
        (true, "tray.clean") => "Clean Safe Items".into(),
        (true, "tray.monitorOn") => "Background Monitor: On".into(),
        (true, "tray.monitorOff") => "Background Monitor: Off".into(),
        (true, "tray.autoOn") => "Auto-Clean: On".into(),
        (true, "tray.autoOff") => "Auto-Clean: Off".into(),
        (true, "tray.autostartOn") => "Launch at Login: On".into(),
        (true, "tray.autostartOff") => "Launch at Login: Off".into(),
        (true, "tray.quit") => "Quit".into(),
        (true, "tray.noScanYet") => "Scan a disk first".into(),
        (true, "tray.nothingSafe") => "No cleanable safe items right now".into(),
        (true, "tray.cleanDone") => "Freed {0} (files are in Trash, recoverable)".into(),
        (true, "tray.cleanFailed") => "Cleanup failed: {0}".into(),
        (true, "tray.scanning") => "Scanning · {0} items".into(),
        (true, "tray.pausedTip") => "Scan paused · {0} items".into(),
        (false, "tray.tooltip") => "Sift 磁盘空间管理".into(),
        (false, "tray.open") => "打开 Sift".into(),
        (false, "tray.scan") => "立即扫描".into(),
        (false, "tray.cancelScan") => "取消扫描".into(),
        (false, "tray.pause") => "暂停扫描".into(),
        (false, "tray.resume") => "继续扫描".into(),
        (false, "tray.clean") => "清理安全项".into(),
        (false, "tray.monitorOn") => "后台监控：开".into(),
        (false, "tray.monitorOff") => "后台监控：关".into(),
        (false, "tray.autoOn") => "自动整理：开".into(),
        (false, "tray.autoOff") => "自动整理：关".into(),
        (false, "tray.autostartOn") => "开机自启：开".into(),
        (false, "tray.autostartOff") => "开机自启：关".into(),
        (false, "tray.quit") => "退出".into(),
        (false, "tray.noScanYet") => "请先扫描磁盘".into(),
        (false, "tray.nothingSafe") => "当前没有可清理的安全项".into(),
        (false, "tray.cleanDone") => "已释放 {0}（文件在回收站，可恢复）".into(),
        (false, "tray.cleanFailed") => "清理失败：{0}".into(),
        (false, "tray.scanning") => "正在扫描 · 已发现 {0} 项".into(),
        (false, "tray.pausedTip") => "扫描已暂停 · 已发现 {0} 项".into(),
        _ => key.to_owned().into(),
    }
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "open" => show_window(app),
        "scan" => trigger_scan(app),
        "pause" => pause_scan(app),
        "resume" => resume_scan(app),
        "cancel" => cancel_scan(app),
        "clean" => clean_safe_items(app),
        "auto" => toggle_auto(app),
        "monitor" => toggle_monitor(app),
        "autostart" => toggle_autostart(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false) {
            let _ = window.hide();
        } else {
            show_window(app);
        }
    }
}

/// Start a scan of the last-scanned root, falling back to the system volume.
fn trigger_scan(app: &AppHandle) {
    let manager = app.state::<crate::scanner::ScanManager>();
    if manager.is_running() {
        return;
    }

    let root = manager.last_root().or_else(|| {
        sift_platform::volume::list_volumes()
            .into_iter()
            .find(|volume| volume.is_system())
            .map(|volume| volume.mount_point.to_path_buf())
    });

    let Some(root) = root else {
        show_window(app);
        return;
    };
    let root_text = root.to_string_lossy().to_string();
    if crate::scanner::start_scan(app.clone(), manager, root_text.clone(), root_text).is_ok() {
        spawn_menu_refresh(app);
    }
}

fn pause_scan(app: &AppHandle) {
    app.state::<crate::scanner::ScanManager>().pause_current();
    sync_menu_state(app);
}

fn resume_scan(app: &AppHandle) {
    app.state::<crate::scanner::ScanManager>().resume_current();
    sync_menu_state(app);
}

fn cancel_scan(app: &AppHandle) {
    app.state::<crate::scanner::ScanManager>().cancel_current();
}

/// Analyse the last scan (if any) and move every remembered safe item inside the
/// home directory to the trash. The same guardrails the UI and the scheduler
/// use apply here, and the outcome is reported as a native notification.
fn clean_safe_items(app: &AppHandle) {
    let services = app.state::<crate::analyze::AppServices>();
    let manager = app.state::<crate::scanner::ScanManager>();

    if manager.is_running() {
        return;
    }

    if manager.last_tree().is_some() {
        if let Err(error) = crate::analyze::analyze_current_with(&services, &manager) {
            notify(app, "tray.cleanFailed", &error);
            return;
        }
    }

    let home = dirs::home_dir();
    let cleanable = services.store.cleanable();
    let entries: Vec<_> = cleanable
        .entries()
        .iter()
        .filter(|entry| entry.safety == Safety::Safe)
        .filter(|entry| {
            home.as_deref()
                .map(|home| entry.path.starts_with(home))
                .unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        let key = if manager.last_tree().is_some() {
            "tray.nothingSafe"
        } else {
            "tray.noScanYet"
        };
        notify(app, key, "");
        return;
    }

    let paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.to_string_lossy().into_owned())
        .collect();
    let results = crate::analyze::perform_cleanup(&services, &paths, false);

    let freed_bytes: u64 = entries
        .iter()
        .zip(results.iter())
        .filter(|(_, result)| result.ok)
        .map(|(entry, _)| entry.size)
        .sum();
    notify(app, "tray.cleanDone", &sift_core::format_bytes(freed_bytes));
    sync_menu_state(app);
}

fn toggle_auto(app: &AppHandle) {
    let services = app.state::<crate::analyze::AppServices>();
    let next_is_auto = !services.store.settings().monitor.auto_mode.may_delete();

    let new_mode = if next_is_auto {
        sift_store::AutoCleanMode::AutoApproved
    } else {
        sift_store::AutoCleanMode::NotifyOnly
    };
    services
        .store
        .update_settings(|settings| settings.monitor.auto_mode = new_mode);
    let _ = services.store.flush_if_dirty();

    if next_is_auto && !services.monitor_running() {
        let manager = app.state::<crate::scanner::ScanManager>();
        let _ = crate::analyze::start_monitor_with(app, &services, &manager);
    }
    sync_menu_state(app);
}

fn toggle_monitor(app: &AppHandle) {
    let services = app.state::<crate::analyze::AppServices>();
    if services.monitor_running() {
        if let Some(control) = services.monitor.lock().unwrap().take() {
            control.cancel();
        }
        sync_menu_state(app);
        return;
    }

    let manager = app.state::<crate::scanner::ScanManager>();
    if let Err(error) = crate::analyze::start_monitor_with(app, &services, &manager) {
        notify(app, "tray.cleanFailed", &error);
        return;
    }
    sync_menu_state(app);
}

fn toggle_autostart(app: &AppHandle) {
    let manager = app.autolaunch();
    let enabled = manager.is_enabled().unwrap_or(false);
    let result = if enabled {
        manager.disable()
    } else {
        manager.enable()
    };
    if result.is_ok() {
        sync_menu_state(app);
    }
}

/// Rebuild the tray menu so its labels match the new runtime state.
fn sync_menu_state(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("sift-tray") else {
        return;
    };
    if let Ok(menu) = build_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// While a scan runs, periodically refresh the menu and tooltip so progress is
/// visible without opening the window (R7.1).
fn spawn_menu_refresh(app: &AppHandle) {
    let manager = app.state::<crate::scanner::ScanManager>();
    if !manager.try_start_menu_refresh() {
        return;
    }

    let handle = app.clone();
    std::thread::Builder::new()
        .name("sift-tray-refresh".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(1500));
                let manager = handle.state::<crate::scanner::ScanManager>();
                if !manager.is_running() {
                    manager.finish_menu_refresh();
                    sync_menu_state(&handle);
                    return;
                }

                let Some(tray) = handle.tray_by_id("sift-tray") else {
                    continue;
                };
                let count = manager.file_count().to_string();
                let key = if manager.is_paused() {
                    "tray.pausedTip"
                } else {
                    "tray.scanning"
                };
                let tooltip = tray_text(&handle, key).replace("{0}", &count);
                let _ = tray.set_tooltip(Some(tooltip));
                if let Ok(menu) = build_menu(&handle) {
                    let _ = tray.set_menu(Some(menu));
                }
            }
        })
        .expect("spawn tray refresh");
}

/// Show a native notification with a tray message, substituting `{0}`.
fn notify(app: &AppHandle, key: &str, param: &str) {
    let body = tray_text(app, key).replace("{0}", param);
    let _ = app
        .notification()
        .builder()
        .title("Sift")
        .body(body)
        .show();
}
