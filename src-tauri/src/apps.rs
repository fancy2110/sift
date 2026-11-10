//! Tauri adapters for the environment inventory and app uninstall planning.
//!
//! Inventory and planning are read-only. Execution is deliberately not exposed
//! here yet: it must go through the shared trash/quarantine layer with an
//! explicit confirmation and an undo manifest.

use serde::Serialize;
use std::path::PathBuf;
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
    Ok(sift_apps::plan_uninstall(&app, &home))
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
    let plan = sift_apps::plan_uninstall(&app, &home);
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
