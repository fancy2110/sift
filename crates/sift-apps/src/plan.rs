//! Read-only uninstall planning.
//!
//! This module finds the conventional leftovers of an app and classifies them.
//! It deliberately does not delete, trash or unload anything: execution lives
//! behind the explicit confirmation flow and the shared trash/quarantine layer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::context::AppFact;
use crate::recipes::RecipeArtifact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    Bundle,
    Support,
    Cache,
    Preferences,
    Container,
    GroupContainer,
    SavedState,
    Log,
    LaunchAgent,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactScope {
    User,
    System,
    Shared,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Disposition {
    AutoSafe,
    Review,
    Keep,
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallArtifact {
    pub path: PathBuf,
    pub kind: ArtifactKind,
    pub scope: ArtifactScope,
    pub disposition: Disposition,
    pub exists: bool,
    pub size_bytes: Option<u64>,
    pub requires_admin: bool,
    /// Other installed apps that also appear to use this path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
    /// Why it is in the plan: `bundle`, `convention`, `recipe:homebrew`, `user`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUninstallPlan {
    pub app_key: String,
    pub app_name: String,
    pub artifacts: Vec<UninstallArtifact>,
    pub total_bytes: u64,
    pub warnings: Vec<String>,
}

/// Build a plan from conventional macOS locations. Paths that do not exist are
/// kept in the plan with `exists: false` only for the app bundle itself; other
/// missing locations are omitted so the UI does not show noise.
pub fn plan_uninstall(app: &AppFact, home: &Path) -> AppUninstallPlan {
    plan_uninstall_with_apps(app, home, &[])
}

/// Build a plan and mark paths that another installed app appears to share.
pub fn plan_uninstall_with_apps(
    app: &AppFact,
    home: &Path,
    all_apps: &[AppFact],
) -> AppUninstallPlan {
    plan_uninstall_with_recipes(app, home, all_apps, &[])
}

/// Build a plan and merge curated recipe paths (currently Homebrew cask zap).
pub fn plan_uninstall_with_recipes(
    app: &AppFact,
    home: &Path,
    all_apps: &[AppFact],
    recipes: &[RecipeArtifact],
) -> AppUninstallPlan {
    let bundle = path_from_display(&app.install_path, home);
    let bundle_id = app.bundle_id.as_deref();
    let name = &app.name;

    let mut candidates: Vec<(PathBuf, ArtifactKind, Disposition)> = Vec::new();
    candidates.push((bundle, ArtifactKind::Bundle, Disposition::Review));

    if let Some(bundle_id) = bundle_id {
        candidates.push((
            home.join("Library/Application Support").join(bundle_id),
            ArtifactKind::Support,
            Disposition::Review,
        ));
        candidates.push((
            home.join("Library/Caches").join(bundle_id),
            ArtifactKind::Cache,
            Disposition::AutoSafe,
        ));
        candidates.push((
            home.join("Library/Preferences")
                .join(format!("{bundle_id}.plist")),
            ArtifactKind::Preferences,
            Disposition::Review,
        ));
        candidates.push((
            home.join("Library/Containers").join(bundle_id),
            ArtifactKind::Container,
            Disposition::Review,
        ));
        candidates.push((
            home.join("Library/Saved Application State")
                .join(format!("{bundle_id}.savedState")),
            ArtifactKind::SavedState,
            Disposition::AutoSafe,
        ));
        candidates.push((
            home.join("Library/LaunchAgents")
                .join(format!("{bundle_id}.plist")),
            ArtifactKind::LaunchAgent,
            Disposition::Review,
        ));
    }

    candidates.push((
        home.join("Library/Application Support").join(name),
        ArtifactKind::Support,
        Disposition::Review,
    ));
    candidates.push((
        home.join("Library/Caches").join(name),
        ArtifactKind::Cache,
        Disposition::AutoSafe,
    ));
    candidates.push((
        home.join("Library/Logs").join(name),
        ArtifactKind::Log,
        Disposition::Review,
    ));

    let mut seen = std::collections::BTreeSet::new();
    let mut artifacts = Vec::new();
    let mut total = 0u64;
    for (path, kind, disposition) in candidates {
        if !seen.insert(path.clone()) {
            continue;
        }
        let exists = path.exists();
        if !exists && kind != ArtifactKind::Bundle {
            continue;
        }
        let size = if exists { size_of(&path) } else { None };
        let shared_with = shared_with(&path, app, all_apps);
        let disposition = if !shared_with.is_empty() && disposition == Disposition::AutoSafe {
            Disposition::Review
        } else {
            disposition
        };
        total = total.saturating_add(size.unwrap_or(0));
        artifacts.push(UninstallArtifact {
            path,
            kind,
            scope: ArtifactScope::User,
            disposition,
            exists,
            size_bytes: size,
            requires_admin: false,
            shared_with,
            source: if kind == ArtifactKind::Bundle {
                "bundle"
            } else {
                "convention"
            }
            .to_string(),
        });
    }

    for recipe in recipes {
        if let Some(existing) = artifacts
            .iter_mut()
            .find(|artifact| artifact.path == recipe.path)
        {
            // A recipe is stronger evidence than a convention; keep the
            // existing disposition/size but credit the source.
            existing.source = recipe.source.clone();
            continue;
        }
        if !seen.insert(recipe.path.clone()) {
            continue;
        }
        let exists = recipe.path.exists();
        let size = if exists { size_of(&recipe.path) } else { None };
        let shared_with = shared_with(&recipe.path, app, all_apps);
        let disposition = match recipe.kind {
            ArtifactKind::Cache | ArtifactKind::SavedState if shared_with.is_empty() => {
                Disposition::AutoSafe
            }
            ArtifactKind::GroupContainer => Disposition::Keep,
            _ => Disposition::Review,
        };
        total = total.saturating_add(size.unwrap_or(0));
        artifacts.push(UninstallArtifact {
            path: recipe.path.clone(),
            kind: recipe.kind,
            scope: ArtifactScope::User,
            disposition,
            exists,
            size_bytes: size,
            requires_admin: false,
            shared_with,
            source: recipe.source.clone(),
        });
    }

    let mut warnings = Vec::new();
    if artifacts
        .iter()
        .any(|artifact| artifact.disposition == Disposition::Review)
    {
        warnings.push("warn.reviewRemoval".to_string());
    }

    AppUninstallPlan {
        app_key: app.key.clone(),
        app_name: app.name.clone(),
        artifacts,
        total_bytes: total,
        warnings,
    }
}

fn shared_with(path: &Path, app: &AppFact, all_apps: &[AppFact]) -> Vec<String> {
    let haystack = path.to_string_lossy().to_ascii_lowercase();
    let mut shared = Vec::new();
    for other in all_apps {
        if other.key == app.key {
            continue;
        }
        let name = other.name.trim().to_ascii_lowercase();
        let bundle_match = other
            .bundle_id
            .as_deref()
            .is_some_and(|id| haystack.contains(&id.to_ascii_lowercase()));
        let name_match = name.len() >= 3 && haystack.contains(&name);
        if bundle_match || name_match {
            shared.push(other.key.clone());
        }
    }
    shared
}

fn path_from_display(text: &str, home: &Path) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else if text == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(text)
    }
}

/// Bounded recursive size. Symlinks are not followed; the cap keeps a runaway
/// tree from turning a plan into a full scan.
fn size_of(path: &Path) -> Option<u64> {
    const MAX_ENTRIES: usize = 10_000;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_file() {
        return Some(meta.len());
    }
    if !meta.is_dir() {
        return Some(0);
    }
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Some(total);
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AppFact;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-apps-plan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(name: &str, bundle: &str) -> AppFact {
        AppFact {
            key: bundle.to_string(),
            name: name.to_string(),
            version: Some("1.0".into()),
            bundle_id: Some(bundle.to_string()),
            install_path: format!("/Applications/{name}.app"),
            last_used_days: None,
            artifact_count: 0,
        }
    }

    #[test]
    fn conventional_paths_are_classified() {
        let home = temp_home("conventional");
        std::fs::create_dir_all(home.join("Library/Caches/com.example.app")).unwrap();
        std::fs::write(
            home.join("Library/Caches/com.example.app/blob"),
            vec![7u8; 100],
        )
        .unwrap();
        std::fs::create_dir_all(home.join("Library/Application Support/com.example.app")).unwrap();

        let plan = plan_uninstall(&app("Example", "com.example.app"), &home);
        let cache = plan
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == ArtifactKind::Cache && artifact.exists)
            .expect("cache artifact");
        assert_eq!(cache.disposition, Disposition::AutoSafe);
        let support = plan
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == ArtifactKind::Support && artifact.exists)
            .expect("support artifact");
        assert_eq!(support.disposition, Disposition::Review);
        assert!(plan.total_bytes >= 100);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn shared_paths_are_downgraded_to_review() {
        let home = temp_home("shared");
        std::fs::create_dir_all(home.join("Library/Caches/com.example.app")).unwrap();
        let current = app("Example", "com.example.app");
        let other = app("Example Helper", "com.example");
        let plan = plan_uninstall_with_apps(&current, &home, &[current.clone(), other]);
        let cache = plan
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == ArtifactKind::Cache && artifact.exists)
            .expect("cache");
        assert_eq!(cache.disposition, Disposition::Review);
        assert!(!cache.shared_with.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn recipe_paths_are_merged_and_deduped() {
        let home = temp_home("recipe");
        let cache = home.join("Library/Caches/com.example.app");
        std::fs::create_dir_all(&cache).unwrap();
        let app = app("Example", "com.example.app");
        let recipe = crate::recipes::RecipeArtifact {
            path: cache.clone(),
            kind: ArtifactKind::Cache,
            source: "recipe:homebrew".into(),
        };
        let plan = plan_uninstall_with_recipes(&app, &home, &[], &[recipe]);
        let matches: Vec<_> = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.path == cache)
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source, "recipe:homebrew");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_paths_are_omitted_except_the_bundle() {
        let home = temp_home("missing");
        let plan = plan_uninstall(&app("Ghost", "com.example.ghost"), &home);
        assert_eq!(plan.artifacts.len(), 1);
        assert_eq!(plan.artifacts[0].kind, ArtifactKind::Bundle);
        assert!(!plan.artifacts[0].exists);
        let _ = std::fs::remove_dir_all(&home);
    }
}
