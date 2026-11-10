//! Tauri adapters for the environment inventory and app uninstall planning.
//!
//! Inventory and planning are read-only. Execution is deliberately not exposed
//! here yet: it must go through the shared trash/quarantine layer with an
//! explicit confirmation and an undo manifest.

use notify::Watcher as _;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager as _;
use tauri::State;

use crate::analyze::AppServices;
use crate::scanner::ScanManager;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDto {
    pub updated_at_ms: i64,
    pub apps: Vec<sift_apps::AppFact>,
    pub package_managers: Vec<sift_apps::PmFact>,
    pub projects: Vec<sift_apps::ProjectFact>,
    pub hints: Vec<sift_apps::MaintenanceHint>,
}

impl EnvironmentDto {
    fn from_doc(doc: sift_store::EnvironmentDoc) -> Self {
        Self {
            updated_at_ms: doc.updated_at_ms,
            apps: doc.context.apps,
            package_managers: doc.context.package_managers,
            projects: doc.context.projects,
            hints: doc.hints,
        }
    }
}

/// The last persisted environment snapshot.
#[tauri::command]
pub fn environment_summary(services: State<'_, AppServices>) -> EnvironmentDto {
    EnvironmentDto::from_doc(services.store.environment())
}

/// Rebuild the environment snapshot on demand (also done after each analysis).
#[tauri::command]
pub fn refresh_environment(
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> EnvironmentDto {
    let roots: Vec<std::path::PathBuf> = manager.last_root().into_iter().collect();
    let context = sift_apps::detect(&roots, dirs::home_dir().as_deref());
    let hints = sift_apps::hints_for(&context);
    let now = now_ms();
    services
        .environment_dirty
        .store(false, std::sync::atomic::Ordering::Relaxed);
    services.store.record_environment(context, hints, now);
    let _ = services.store.flush_if_dirty();
    EnvironmentDto::from_doc(services.store.environment())
}

/// Build a read-only uninstall plan for one detected application.
#[tauri::command]
pub fn plan_app_uninstall(
    services: State<'_, AppServices>,
    app_key: String,
) -> Result<sift_apps::AppUninstallPlan, String> {
    let app = find_app(&services, &app_key)?;
    let home = dirs::home_dir().ok_or_else(|| "err.noHome".to_string())?;
    let doc = services.store.environment();
    let recipes = sift_apps::homebrew_zap_paths(&app, &home);
    Ok(sift_apps::plan_uninstall_with_recipes(
        &app,
        &home,
        &doc.context.apps,
        &recipes,
    ))
}

/// Move the selected artifacts into Sift's quarantine and return a manifest.
///
/// The plan is rebuilt from the stored environment; every path is validated
/// against it, so the front end cannot delete an arbitrary path.
#[tauri::command]
pub fn execute_app_uninstall(
    services: State<'_, AppServices>,
    app_key: String,
    paths: Vec<String>,
) -> Result<sift_apps::UninstallManifest, String> {
    let app = find_app(&services, &app_key)?;
    let home = dirs::home_dir().ok_or_else(|| "err.noHome".to_string())?;
    let doc = services.store.environment();
    let recipes = sift_apps::homebrew_zap_paths(&app, &home);
    let plan = sift_apps::plan_uninstall_with_recipes(&app, &home, &doc.context.apps, &recipes);
    let quarantine = quarantine_root(&services)?;
    let selected: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    sift_apps::execute_uninstall(&plan, &selected, &quarantine)
}

/// Restore a previous uninstall from its manifest.
#[tauri::command]
pub fn restore_app_uninstall(
    services: State<'_, AppServices>,
    manifest_id: String,
) -> Result<sift_apps::RestoreReport, String> {
    let root = quarantine_root(&services)?;
    let path = root.join(&manifest_id).join("manifest.json");
    let json = std::fs::read_to_string(&path).map_err(|err| format!("read manifest: {err}"))?;
    let manifest: sift_apps::UninstallManifest =
        serde_json::from_str(&json).map_err(|err| format!("parse manifest: {err}"))?;
    sift_apps::restore_uninstall(&manifest)
}

/// Look up the static command plan for a package-manager action.
#[tauri::command]
pub fn plan_pm_action(
    services: State<'_, AppServices>,
    pm_id: String,
    action: String,
) -> Result<Option<sift_apps::PmActionPlan>, String> {
    if !pm_detected(&services, &pm_id) {
        return Ok(None);
    }
    Ok(sift_apps::plan_action(&pm_id, &action))
}

/// Run a catalogued package-manager action. No program, argument or shell is
/// accepted from the caller; only the `(pm_id, action)` pair is.
#[tauri::command]
pub fn run_pm_action(
    services: State<'_, AppServices>,
    pm_id: String,
    action: String,
) -> Result<sift_apps::PmRunReport, String> {
    if !pm_detected(&services, &pm_id) {
        return Err("err.pmNotFound".to_string());
    }
    let plan = sift_apps::plan_action(&pm_id, &action)
        .ok_or_else(|| "err.unsupportedAction".to_string())?;
    let report = sift_apps::run_action(&plan)?;
    let status = if report.success {
        sift_apps::HintStatus::Validated
    } else {
        sift_apps::HintStatus::Stale
    };
    let _ = services
        .store
        .set_hint_status(&format!("pm:{pm_id}:{action}"), status);
    let _ = services.store.flush_if_dirty();
    Ok(report)
}

fn pm_detected(services: &AppServices, pm_id: &str) -> bool {
    services
        .store
        .environment()
        .context
        .package_managers
        .iter()
        .any(|pm| pm.id == pm_id && pm.detected)
}

fn find_app(services: &AppServices, app_key: &str) -> Result<sift_apps::AppFact, String> {
    let doc = services.store.environment();
    doc.context
        .apps
        .iter()
        .find(|app| app.key == app_key)
        .cloned()
        .ok_or_else(|| "err.appNotFound".to_string())
}

fn quarantine_root(services: &AppServices) -> Result<PathBuf, String> {
    services
        .store
        .root()
        .map(|root| root.join("Quarantine"))
        .ok_or_else(|| "err.noStoreRoot".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Watch the application directories and mark the environment cache dirty.
/// Inventory itself runs on the analysis thread; this only flips a flag.
pub fn start_environment_watch(app: tauri::AppHandle) {
    let Some(services) = app.try_state::<AppServices>() else {
        return;
    };
    let dirty = Arc::clone(&services.environment_dirty);
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
        #[cfg(target_os = "linux")]
        {
            dirs.push(PathBuf::from("/usr/share/applications"));
            dirs.push(home.join(".local/share/applications"));
        }
        #[cfg(windows)]
        {
            if let Some(local) = dirs::data_local_dir() {
                dirs.push(local.join("Programs"));
            }
            dirs.push(PathBuf::from(r"C:\Program Files"));
            dirs.push(PathBuf::from(r"C:\Program Files (x86)"));
        }
    }
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        }) else {
            return;
        };
        for dir in dirs {
            if dir.exists() {
                let _ = watcher.watch(&dir, notify::RecursiveMode::NonRecursive);
            }
        }
        while rx.recv().is_ok() {
            dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}
