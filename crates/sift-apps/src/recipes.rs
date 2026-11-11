//! Curated uninstall recipes, starting with Homebrew Cask `zap` data.
//!
//! Homebrew's cask definitions already describe where an app leaves data on
//! disk. We read the installed cask metadata on demand (one `brew info` for the
//! app being planned), normalize its paths and feed them into the planner. The
//! recipe is evidence, not an instruction: shared detection and the normal
//! disposition rules still apply.

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::context::AppFact;
use crate::plan::ArtifactKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeArtifact {
    pub path: PathBuf,
    pub kind: ArtifactKind,
    pub source: String,
}

/// Read Homebrew cask `zap` paths for an app. Returns an empty list when
/// Homebrew is absent, the cask is not found, or the probe times out.
pub fn homebrew_zap_paths(app: &AppFact, home: &Path) -> Vec<RecipeArtifact> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, home);
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        let Some(json) = probe_cask(app) else {
            return Vec::new();
        };
        parse_zap_json(&json, home)
    }
}

#[cfg(target_os = "macos")]
fn probe_cask(app: &AppFact) -> Option<String> {
    let slug = cask_slug(&app.name);
    let candidates = [
        slug.clone(),
        app.bundle_id
            .as_deref()
            .map(|id| id.to_ascii_lowercase())
            .unwrap_or_default(),
    ];
    for candidate in candidates.iter().filter(|c| !c.is_empty()) {
        let output = Command::new("brew")
            .args(["info", "--cask", "--json=v2", candidate])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if text.contains("\"zap\"") || text.contains("\"artifacts\"") {
                return Some(text);
            }
        }
    }
    None
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cask_slug(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse the `zap` section of `brew info --cask --json=v2`.
///
/// Exposed for tests; the real caller only ever passes the output of a local
/// `brew` probe.
pub fn parse_zap_json(text: &str, home: &Path) -> Vec<RecipeArtifact> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(cask) = value
        .get("casks")
        .and_then(|casks| casks.as_array())
        .and_then(|casks| casks.first())
    else {
        return Vec::new();
    };
    let Some(zap) = cask.get("zap").and_then(|zap| zap.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in zap {
        let Some(object) = entry.as_object() else {
            continue;
        };
        for key in ["trash", "rmdir", "delete"] {
            let Some(paths) = object.get(key).and_then(|value| value.as_array()) else {
                continue;
            };
            for value in paths {
                let Some(raw) = value.as_str() else {
                    continue;
                };
                let Some(path) = expand_recipe_path(raw, home) else {
                    continue;
                };
                if !path.starts_with(home) {
                    // System-level leftovers need admin and are out of scope.
                    continue;
                }
                if !seen.insert(path.clone()) {
                    continue;
                }
                out.push(RecipeArtifact {
                    kind: classify(&path),
                    path,
                    source: "recipe:homebrew".to_string(),
                });
            }
        }
    }
    out
}

fn expand_recipe_path(raw: &str, home: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let expanded = raw
        .replace("$HOME", &home.to_string_lossy())
        .replace("${HOME}", &home.to_string_lossy());
    if let Some(rest) = expanded.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    if expanded == "~" {
        return Some(home.to_path_buf());
    }
    Some(PathBuf::from(expanded))
}

fn classify(path: &Path) -> ArtifactKind {
    let text = path.to_string_lossy();
    if text.contains("/Library/Caches/") {
        ArtifactKind::Cache
    } else if text.contains("/Library/Logs/") {
        ArtifactKind::Log
    } else if text.contains("/Library/Preferences/") {
        ArtifactKind::Preferences
    } else if text.contains("/Library/Group Containers/") {
        ArtifactKind::GroupContainer
    } else if text.contains("/Library/Containers/") {
        ArtifactKind::Container
    } else if text.contains("/Library/Application Support/") {
        ArtifactKind::Support
    } else if text.contains("/Library/Saved Application State/") {
        ArtifactKind::SavedState
    } else if text.contains("/Library/LaunchAgents/") {
        ArtifactKind::LaunchAgent
    } else {
        ArtifactKind::Other
    }
}

/// Timeout used by the caller when invoking a probe; kept public so the Tauri
/// layer can share the same budget.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "casks": [{
        "token": "example",
        "zap": [
          {"trash": ["~/Library/Application Support/com.example.app", "~/Library/Caches/com.example.app"]},
          {"rmdir": ["~/Library/Logs/Example"]},
          {"delete": ["/Library/LaunchDaemons/com.example.helper.plist"]}
        ]
      }]
    }"#;

    #[cfg(unix)]
    #[test]
    fn parses_trash_rmdir_and_delete() {
        let home = PathBuf::from("/Users/alice");
        let artifacts = parse_zap_json(FIXTURE, &home);
        assert_eq!(artifacts.len(), 3);
        assert!(artifacts.iter().any(|a| a.kind == ArtifactKind::Support));
        assert!(artifacts.iter().any(|a| a.kind == ArtifactKind::Cache));
        assert!(artifacts.iter().any(|a| a.kind == ArtifactKind::Log));
        // The system-level LaunchDaemon is outside $HOME and is skipped.
        assert!(!artifacts.iter().any(|a| a.path.starts_with("/Library")));
    }

    #[test]
    fn cask_slug_normalizes_names() {
        assert_eq!(cask_slug("Google Chrome"), "google-chrome");
        assert_eq!(cask_slug("Visual Studio Code"), "visual-studio-code");
        assert_eq!(cask_slug("1Password"), "1password");
    }
}
