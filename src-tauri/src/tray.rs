//! System tray: open the window, start a scan, toggle unattended cleanup and
//! autostart, or quit. Closing the window hides it here instead of exiting.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use tauri_plugin_autostart::ManagerExt;

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

/// Build the menu with labels reflecting the current auto/autostart state.
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::DynRuntime>> {
    let services = app.state::<crate::analyze::AppServices>();
    let auto_on = services.store.settings().monitor.auto_mode.may_delete();
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);

    let open = MenuItem::with_id(app, "open", tray_text(app, "tray.open"), true, None::<&str>)?;
    let scan = MenuItem::with_id(app, "scan", tray_text(app, "tray.scan"), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
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
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", tray_text(app, "tray.quit"), true, None::<&str>)?;

    Menu::with_items(app, &[&open, &scan, &sep1, &auto, &autostart, &sep2, &quit])
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
        (true, "tray.autoOn") => "Auto-Clean: On".into(),
        (true, "tray.autoOff") => "Auto-Clean: Off".into(),
        (true, "tray.autostartOn") => "Launch at Login: On".into(),
        (true, "tray.autostartOff") => "Launch at Login: Off".into(),
        (true, "tray.quit") => "Quit".into(),
        (false, "tray.tooltip") => "Sift 磁盘空间管理".into(),
        (false, "tray.open") => "打开 Sift".into(),
        (false, "tray.scan") => "立即扫描".into(),
        (false, "tray.autoOn") => "自动整理：开".into(),
        (false, "tray.autoOff") => "自动整理：关".into(),
        (false, "tray.autostartOn") => "开机自启：开".into(),
        (false, "tray.autostartOff") => "开机自启：关".into(),
        (false, "tray.quit") => "退出".into(),
        _ => key.to_owned().into(),
    }
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "open" => show_window(app),
        "scan" => trigger_scan(app),
        "auto" => toggle_auto(app),
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

/// Start a scan of the last-scanned root, or cancel a running one. The webview
/// stays alive while hidden, so its store still receives the scan and analysis.
fn trigger_scan(app: &AppHandle) {
    let manager = app.state::<crate::scanner::ScanManager>();

    if manager.is_running() {
        if let Some(control) = manager.control.lock().unwrap().as_ref() {
            control.cancel();
        }
        return;
    }

    let Some(root) = manager.last_root() else {
        show_window(app);
        return;
    };
    let root_text = root.to_string_lossy().to_string();
    let _ = crate::scanner::start_scan(app.clone(), manager, root_text.clone(), root_text);
    show_window(app);
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

/// Rebuild the tray menu so its labels match the new auto/autostart state.
fn sync_menu_state(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("sift-tray") else {
        return;
    };
    if let Ok(menu) = build_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}
