//! User-defined scheduled cleanup rules: a directory plus a file-name regex.
//!
//! These are deliberately narrow. A rule never deletes a directory, never
//! follows a symbolic link, only matches the *file name*, and every removal
//! still goes through the trash. The command layer validates the regex and
//! the directory before a rule can be saved; the scan itself re-checks that
//! each file is still inside the (canonicalised) directory.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::Serialize;
use sift_store::{CleanupRule, Store};
use tauri::State;

use crate::analyze::AppServices;
use crate::cleanup::DeleteResultItem;

/// A hard stop so one rule cannot turn into an accidental "delete the disk"
/// operation. The scheduler runs unattended, so the bound is not advisory.
const MAX_FILES_PER_RUN: usize = 2_000;
const MAX_BYTES_PER_RUN: u64 = 20 * 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone)]
pub struct ScanMatch {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub matches: Vec<ScanMatch>,
    pub bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreviewDto {
    pub matched: usize,
    pub bytes: u64,
    pub truncated: bool,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleRunDto {
    pub matched: usize,
    pub removed: usize,
    pub failed: usize,
    pub bytes: u64,
    pub truncated: bool,
    pub results: Vec<DeleteResultItem>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.mtime()
            .saturating_mul(1000)
            .saturating_add(meta.mtime_nsec() / 1_000_000)
    }
    #[cfg(not(unix))]
    {
        meta.modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Validate a rule before it can be saved or run.
pub fn validate_rule(rule: &CleanupRule) -> Result<(), String> {
    if rule.dir.trim().is_empty() || rule.pattern.trim().is_empty() {
        return Err("err.ruleIncomplete".into());
    }
    let dir = Path::new(&rule.dir);
    if !dir.is_absolute() {
        return Err("err.ruleDirAbsolute".into());
    }
    let meta = std::fs::metadata(dir).map_err(|_| "err.ruleDirMissing".to_string())?;
    if !meta.is_dir() {
        return Err("err.ruleDirNotDir".into());
    }
    Regex::new(&rule.pattern).map_err(|error| format!("err.ruleRegex|{error}"))?;
    Ok(())
}

/// Collect the files a rule would delete. Pure filesystem work so it can be
/// tested without Tauri state.
pub fn scan_rule(rule: &CleanupRule, now_ms: i64) -> Result<ScanOutcome, String> {
    scan_rule_with_limits(rule, now_ms, MAX_FILES_PER_RUN, MAX_BYTES_PER_RUN)
}

fn scan_rule_with_limits(
    rule: &CleanupRule,
    now_ms: i64,
    max_files: usize,
    max_bytes: u64,
) -> Result<ScanOutcome, String> {
    validate_rule(rule)?;
    let canonical_dir = std::fs::canonicalize(Path::new(&rule.dir))
        .map_err(|_| "err.ruleDirMissing".to_string())?;
    let regex = Regex::new(&rule.pattern).map_err(|error| format!("err.ruleRegex|{error}"))?;
    let cutoff_ms = if rule.older_than_days == 0 {
        None
    } else {
        Some(now_ms.saturating_sub((rule.older_than_days as i64).saturating_mul(86_400_000)))
    };

    let mut outcome = ScanOutcome::default();
    let mut stack: Vec<(PathBuf, usize)> = vec![(canonical_dir.clone(), 0)];
    while let Some((current, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Never follow links: a symlink could point anywhere, including
            // outside the directory the user picked.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !regex.is_match(name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let mtime_ms = modified_ms(&meta);
            if cutoff_ms.is_some_and(|cutoff| mtime_ms > cutoff) {
                continue;
            }
            // Defence in depth: the parent must still canonicalise inside the
            // directory. A rename between listing and deletion is caught here.
            let Some(parent) = path
                .parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
            else {
                continue;
            };
            if !parent.starts_with(&canonical_dir) {
                continue;
            }
            let size = meta.len();
            if outcome.matches.len() >= max_files || outcome.bytes.saturating_add(size) > max_bytes
            {
                outcome.truncated = true;
                break;
            }
            outcome.bytes = outcome.bytes.saturating_add(size);
            outcome.matches.push(ScanMatch { path, size });
        }
        if outcome.truncated {
            break;
        }
    }
    outcome.matches.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(outcome)
}

fn preview(rule: &CleanupRule) -> Result<RulePreviewDto, String> {
    let outcome = scan_rule(rule, now_ms())?;
    Ok(RulePreviewDto {
        matched: outcome.matches.len(),
        bytes: outcome.bytes,
        truncated: outcome.truncated,
        examples: outcome
            .matches
            .iter()
            .take(5)
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect(),
    })
}

/// Scan and trash everything one rule matches. Shared by the "run now" command
/// and the daily scheduler.
pub fn run_rule(store: &Store, rule: &CleanupRule) -> Result<RuleRunDto, String> {
    let outcome = scan_rule(rule, now_ms())?;
    let paths: Vec<PathBuf> = outcome
        .matches
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let trash_results = sift_platform::trash::trash_paths_cancellable(&paths, None);
    let results: Vec<DeleteResultItem> = trash_results
        .iter()
        .map(|result| DeleteResultItem {
            path: result.path.to_string_lossy().into_owned(),
            ok: result.ok,
            error: result.error.clone(),
            cancelled: result.cancelled,
        })
        .collect();
    let mut removed = 0usize;
    let mut failed = 0usize;
    let mut bytes = 0u64;
    let mut titles = Vec::new();
    for (entry, result) in outcome.matches.iter().zip(trash_results.iter()) {
        if result.ok {
            removed += 1;
            bytes = bytes.saturating_add(entry.size);
            if let Some(name) = entry.path.file_name() {
                titles.push(name.to_string_lossy().into_owned());
            }
        } else {
            failed += 1;
        }
    }
    if removed > 0 {
        store.note_cleanup_session(&titles, bytes, true, now_ms());
        let _ = store.flush_if_dirty();
    }
    Ok(RuleRunDto {
        matched: outcome.matches.len(),
        removed,
        failed,
        bytes,
        truncated: outcome.truncated,
        results,
    })
}

// ---- commands --------------------------------------------------------------

#[tauri::command]
pub fn list_cleanup_rules(services: State<'_, AppServices>) -> Vec<CleanupRule> {
    services.store.settings().monitor.cleanup_rules
}

/// Create or update a rule. The id is assigned here when the draft has none.
#[tauri::command]
pub fn save_cleanup_rule(
    services: State<'_, AppServices>,
    rule: CleanupRule,
) -> Result<CleanupRule, String> {
    let mut rule = rule
        .normalized()
        .ok_or_else(|| "err.ruleIncomplete".to_string())?;
    validate_rule(&rule)?;
    if rule.id.is_empty() {
        let seed = format!("{}|{}|{}", rule.dir, rule.pattern, now_ms());
        rule.id = sift_core::NodeKey::from_bytes(seed.as_bytes()).to_string();
    }
    let saved = rule.clone();
    services.store.update_settings(|settings| {
        let list = &mut settings.monitor.cleanup_rules;
        list.retain(|existing| existing.id != saved.id);
        list.push(saved.clone());
    });
    let _ = services.store.flush_if_dirty();
    Ok(rule)
}

#[tauri::command]
pub fn delete_cleanup_rule(services: State<'_, AppServices>, id: String) -> Result<bool, String> {
    let mut removed = false;
    services.store.update_settings(|settings| {
        let before = settings.monitor.cleanup_rules.len();
        settings.monitor.cleanup_rules.retain(|rule| rule.id != id);
        removed = settings.monitor.cleanup_rules.len() != before;
    });
    let _ = services.store.flush_if_dirty();
    Ok(removed)
}

/// Dry run for the form: how many files would this draft match, and how big.
#[tauri::command]
pub fn preview_cleanup_rule(rule: CleanupRule) -> Result<RulePreviewDto, String> {
    let rule = rule
        .normalized()
        .ok_or_else(|| "err.ruleIncomplete".to_string())?;
    preview(&rule)
}

#[tauri::command]
pub fn run_cleanup_rule(
    services: State<'_, AppServices>,
    id: String,
) -> Result<RuleRunDto, String> {
    let rule = services
        .store
        .settings()
        .monitor
        .cleanup_rules
        .into_iter()
        .find(|rule| rule.id == id)
        .ok_or_else(|| "err.ruleNotFound".to_string())?;
    run_rule(&services.store, &rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-rule-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rule(dir: &Path, pattern: &str, older_than_days: u32) -> CleanupRule {
        CleanupRule {
            id: "test".into(),
            name: "test".into(),
            dir: dir.to_string_lossy().into_owned(),
            pattern: pattern.into(),
            older_than_days,
            enabled: false,
        }
    }

    fn age(path: &Path, days: i64) {
        let mtime = SystemTime::now() - std::time::Duration::from_secs((days * 86_400) as u64);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    #[test]
    fn matches_by_name_and_age() {
        let dir = scratch("age");
        let old = dir.join("a.log");
        let old_txt = dir.join("b.txt");
        let fresh = dir.join("c.log");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&old_txt, b"x").unwrap();
        std::fs::write(&fresh, b"x").unwrap();
        age(&old, 10);
        age(&old_txt, 10);

        let outcome = scan_rule(&rule(&dir, r"\.log$", 1), now_ms()).unwrap();
        let names: Vec<String> = outcome
            .matches
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["a.log"]);
    }

    #[test]
    fn walks_subdirectories_but_skips_links() {
        let dir = scratch("links");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("top.log"), b"x").unwrap();
        std::fs::write(dir.join("sub/deep.log"), b"x").unwrap();
        let outside = scratch("outside");
        std::fs::write(outside.join("escape.log"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();

        let outcome = scan_rule(&rule(&dir, r"\.log$", 0), now_ms()).unwrap();
        assert_eq!(outcome.matches.len(), 2);
        // Nothing from the linked directory escaped.
        assert!(outcome
            .matches
            .iter()
            .all(|entry| !entry.path.starts_with(&outside)));
    }

    #[test]
    fn rejects_a_relative_directory() {
        let outcome = scan_rule(&rule(Path::new("relative"), r"\.log$", 0), now_ms());
        assert_eq!(outcome.unwrap_err(), "err.ruleDirAbsolute");
    }

    #[test]
    fn stops_at_the_file_cap() {
        let dir = scratch("cap");
        for index in 0..5 {
            std::fs::write(dir.join(format!("f{index}.log")), b"x").unwrap();
        }
        let outcome =
            scan_rule_with_limits(&rule(&dir, r"\.log$", 0), now_ms(), 2, u64::MAX).unwrap();
        assert_eq!(outcome.matches.len(), 2);
        assert!(outcome.truncated);
    }
}
