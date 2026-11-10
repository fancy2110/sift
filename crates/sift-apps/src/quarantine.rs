//! Reversible uninstall execution.
//!
//! Files are moved into a Sift-owned quarantine directory on the same volume
//! (a rename, not a copy) and a manifest records every original path. Restore
//! is the inverse move. Nothing here is called automatically: the caller must
//! have shown a plan and received explicit confirmation.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::plan::{AppUninstallPlan, Disposition};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub original: PathBuf,
    pub stored: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallManifest {
    pub schema_version: u32,
    pub id: String,
    pub app_key: String,
    pub app_name: String,
    pub created_at_ms: i64,
    pub quarantine_root: PathBuf,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

/// Move the selected artifacts into `quarantine_root/<id>/payload`.
///
/// `selected` must be a subset of the plan's artifact paths; anything else is
/// rejected rather than trusted. Directories that are already gone are skipped.
pub fn execute(
    plan: &AppUninstallPlan,
    selected: &[PathBuf],
    quarantine_root: &Path,
) -> Result<UninstallManifest, String> {
    let allowed: std::collections::HashSet<&Path> = plan
        .artifacts
        .iter()
        .filter(|artifact| artifact.disposition != Disposition::Forbidden)
        .map(|artifact| artifact.path.as_path())
        .collect();

    let id = format!("{}-{}", now_ms(), std::process::id());
    let payload = quarantine_root.join(&id).join("payload");
    std::fs::create_dir_all(&payload).map_err(|err| format!("create quarantine: {err}"))?;

    let mut entries = Vec::new();
    let mut index = 0usize;
    for original in selected {
        if !allowed.contains(original.as_path()) {
            return Err(format!(
                "path is not part of this plan: {}",
                original.display()
            ));
        }
        if !original.exists() {
            continue;
        }
        let Some(name) = original.file_name() else {
            continue;
        };
        let stored = payload.join(format!("{index:04}-{}", name.to_string_lossy()));
        index += 1;
        let is_dir = original.is_dir();
        if let Err(err) = move_path(original, &stored) {
            return Err(format!("move {}: {err}", original.display()));
        }
        entries.push(ManifestEntry {
            original: original.clone(),
            stored,
            is_dir,
        });
    }

    let manifest = UninstallManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        id,
        app_key: plan.app_key.clone(),
        app_name: plan.app_name.clone(),
        created_at_ms: now_ms(),
        quarantine_root: quarantine_root.to_path_buf(),
        entries,
    };
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("serialize manifest: {err}"))?;
    std::fs::write(
        quarantine_root.join(&manifest.id).join("manifest.json"),
        json,
    )
    .map_err(|err| format!("write manifest: {err}"))?;
    Ok(manifest)
}

/// Move every stored entry back. An existing original is never overwritten.
pub fn restore(manifest: &UninstallManifest) -> Result<RestoreReport, String> {
    let mut restored = Vec::new();
    let mut skipped = Vec::new();
    for entry in &manifest.entries {
        if entry.original.exists() {
            skipped.push(entry.original.clone());
            continue;
        }
        if !entry.stored.exists() {
            skipped.push(entry.original.clone());
            continue;
        }
        if let Some(parent) = entry.original.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        move_path(&entry.stored, &entry.original)
            .map_err(|err| format!("restore {}: {err}", entry.original.display()))?;
        restored.push(entry.original.clone());
    }
    Ok(RestoreReport { restored, skipped })
}

fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Different volume: copy then remove. Bounded by the artifact itself,
        // which the user already saw in the plan.
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_recursive(from, to)?;
            if from.is_dir() {
                std::fs::remove_dir_all(from)
            } else {
                std::fs::remove_file(from)
            }
        }
        Err(err) => Err(err),
    }
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AppFact;
    use crate::plan::plan_uninstall;

    fn temp_home(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sift-quarantine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn execute_moves_and_restore_puts_everything_back() {
        let home = temp_home("roundtrip");
        let cache = home.join("Library/Caches/com.example.app");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("blob"), b"payload").unwrap();

        let app = AppFact {
            key: "com.example.app".into(),
            name: "Example".into(),
            version: None,
            bundle_id: Some("com.example.app".into()),
            install_path: "/Applications/Example.app".into(),
            last_used_days: None,
            artifact_count: 0,
        };
        let plan = plan_uninstall(&app, &home);
        let quarantine = home.join("Library/Application Support/Sift/Quarantine");
        let manifest = execute(&plan, &[cache.clone()], &quarantine).unwrap();
        assert!(!cache.exists());
        assert_eq!(manifest.entries.len(), 1);

        let report = restore(&manifest).unwrap();
        assert_eq!(report.restored, vec![cache.clone()]);
        assert_eq!(std::fs::read(cache.join("blob")).unwrap(), b"payload");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn paths_outside_the_plan_are_rejected() {
        let home = temp_home("reject");
        let app = AppFact {
            key: "com.example.app".into(),
            name: "Example".into(),
            version: None,
            bundle_id: None,
            install_path: "/Applications/Example.app".into(),
            last_used_days: None,
            artifact_count: 0,
        };
        let plan = plan_uninstall(&app, &home);
        let quarantine = home.join("q");
        let result = execute(&plan, &[home.join("Documents/private")], &quarantine);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&home);
    }
}
