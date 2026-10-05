//! Daily scheduled cleanup.
//!
//! A small background thread that wakes a few times a minute and, when the
//! local clock reaches the configured hour (default 04:00), runs the same
//! real pipeline the user triggers manually: scan the system volume, analyse
//! it, then send the remembered, individually approved safe items inside the
//! home directory to the trash. Nothing outside home is removed and only Safe
//! entries the user has approved for automatic cleanup qualify, so the safety
//! guardrails are identical to an interactive cleanup.

use std::path::Path;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use sift_analyze::Safety;
use sift_store::CleanableEntry;
use tauri::{AppHandle, Manager};

use crate::analyze::AppServices;
use crate::scanner::ScanManager;

/// Spawn the scheduler thread. Runs for the lifetime of the app.
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    std::thread::Builder::new()
        .name("sift-schedule".into())
        .spawn(move || run(handle))
        .expect("spawn scheduler");
}

fn run(app: AppHandle) {
    let mut last_fired = String::new();

    loop {
        std::thread::sleep(Duration::from_secs(20));

        let services = app.state::<AppServices>();
        let settings = services.store.settings();
        // The schedule must obey the automatic-cleanup switch: with
        // AutoCleanMode off or notify-only it may scan and report, but it
        // never deletes. (Defense in depth; run_daily re-checks below.)
        if !settings.monitor.scheduled_cleanup_enabled
            || !settings.monitor.auto_mode.may_delete()
        {
            continue;
        }

        let now = chrono::Local::now();
        if now.hour() != settings.monitor.scheduled_hour || now.minute() >= 2 {
            continue;
        }

        let day_key = format!("{}-{}", now.year(), now.ordinal());
        if day_key == last_fired {
            continue;
        }
        last_fired = day_key.clone();

        if let Err(error) = run_daily(&app) {
            eprintln!("scheduled cleanup failed: {error}");
        }
    }
}

/// Perform one scheduled scan → analyse → cleanup pass.
fn run_daily(app: &AppHandle) -> Result<(), String> {
    let manager = app.state::<ScanManager>();
    let services = app.state::<AppServices>();

    let system_volume = sift_platform::volume::list_volumes()
        .into_iter()
        .find(|volume| volume.is_system())
        .ok_or_else(|| "err.noDiskSelected".to_string())?;

    let mount = system_volume.mount_point.to_string_lossy().into_owned();
    if !manager.is_running() {
        crate::scanner::start_scan_with(app, &manager, mount.clone(), mount)?;
    }

    let mut waited = 0;
    while manager.is_running() {
        std::thread::sleep(Duration::from_secs(5));
        waited += 5;
        if waited > 60 * 60 {
            return Err("scan timed out".into());
        }
    }

    crate::analyze::analyze_current_with(&services, &manager)?;

    let home = dirs::home_dir();
    let paths = select_approved_paths(services.store.cleanable().entries(), home.as_deref());

    // Re-check at act time: the user may have flipped the switch while the
    // hour-long scan was running.
    let still_allowed = services
        .store
        .settings()
        .monitor
        .auto_mode
        .may_delete();
    if !paths.is_empty() && still_allowed {
        crate::analyze::perform_cleanup(&services, &paths, true);
    }
    Ok(())
}

/// Select remembered entries for unattended scheduled cleanup: `Safe`,
/// individually approved for automatic cleanup, and inside `home`. A missing
/// home proves nothing, so nothing is selected.
pub(crate) fn select_approved_paths(
    entries: &[CleanableEntry],
    home: Option<&Path>,
) -> Vec<String> {
    let Some(home) = home else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| entry.safety == Safety::Safe && entry.approved_for_auto)
        .filter(|entry| entry.path.starts_with(home))
        .map(|entry| entry.path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use sift_analyze::{PathFingerprint, Reason, VerdictSource};
    use sift_core::NodeKey;

    fn entry(name: &str, path: &str, safety: Safety, approved: bool) -> CleanableEntry {
        CleanableEntry {
            fingerprint: PathFingerprint::new(NodeKey::from_bytes(name.as_bytes()), 1, 0, true),
            path: PathBuf::from(path),
            display_path: path.to_string(),
            name: name.to_string(),
            kind_token: "rebuildableCache".into(),
            size: 1,
            safety,
            confidence: 0.9,
            reason: Reason::key("reason.rebuildableCache"),
            source: VerdictSource::rule("dir.node_modules"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: approved,
            cleanup_command: None,
            cleanup_method: "trashItem".into(),
            impact: Reason::key("reason.rebuildableCache"),
        }
    }

    #[test]
    fn only_safe_approved_entries_inside_home_are_selected() {
        let home = Path::new("/Users/me");
        let entries = vec![
            entry("a", "/Users/me/project/a", Safety::Safe, true),
            entry("b", "/Users/me/project/b", Safety::Safe, false),
            entry("c", "/Users/me/project/c", Safety::Review, true),
            entry("d", "/srv/d", Safety::Safe, true),
        ];
        let paths = select_approved_paths(&entries, Some(home));
        assert_eq!(paths, vec!["/Users/me/project/a".to_string()]);
    }

    #[test]
    fn missing_home_selects_nothing() {
        let entries = vec![entry("a", "/Users/me/a", Safety::Safe, true)];
        assert!(select_approved_paths(&entries, None).is_empty());
    }
}
