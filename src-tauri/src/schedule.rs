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

/// What the scheduler should do on one wake-up. Kept pure so missed-slot
/// handling can be tested without a clock or a running app.
#[derive(Debug, PartialEq, Eq)]
enum SchedAction {
    /// Nothing due (switch off, wrong hour, or already fired for this slot).
    Wait,
    /// The current local day's slot is due now.
    RunToday { day_key: String },
    /// The app was closed (or asleep) through the slot and restarted inside
    /// the catch-up window; run once and stamp the missed day.
    RunMissed { day_key: String },
    /// The missed slot is older than the catch-up window; stamp it so the
    /// same day is not reconsidered, but do not run an unattended cleanup.
    SkipMissed { day_key: String },
}

/// Decide the scheduler action.
///
/// * `now_ordinal` / `now_hour` — local calendar day key (`year-ordinal`) and
///   hour of the wake-up.
/// * `scheduled_hour` — configured slot hour.
/// * `catchup_hours` — how long after the slot a missed run may still be made
///   up; 0 disables catch-up.
/// * `last_run` — persisted day key of the previous successful fire.
fn decide(
    now_ordinal: i32,
    now_hour: u32,
    scheduled_hour: u32,
    catchup_hours: u32,
    last_run: Option<&str>,
) -> SchedAction {
    let day_key = ordinal_key(now_ordinal);

    // Already handled today.
    if last_run == Some(day_key.as_str()) {
        return SchedAction::Wait;
    }

    if now_hour == scheduled_hour {
        return SchedAction::RunToday { day_key };
    }

    if now_hour > scheduled_hour {
        // Today's slot passed while the process was not alive (or not firing).
        // Make it up once if still inside the catch-up window.
        let hours_after_slot = now_hour - scheduled_hour;
        if catchup_hours > 0 && hours_after_slot <= catchup_hours {
            return SchedAction::RunMissed { day_key };
        }
        if catchup_hours == 0 || hours_after_slot > catchup_hours {
            return SchedAction::SkipMissed { day_key };
        }
    }

    // now_hour < scheduled_hour: the slot is still ahead today.
    SchedAction::Wait
}

/// Convert a CE-ordinal day number to the persisted `year-ordinal` key.
fn ordinal_key(ordinal: i32) -> String {
    chrono::NaiveDate::from_num_days_from_ce_opt(ordinal)
        .map(|date| format!("{}-{}", date.year(), date.ordinal()))
        .unwrap_or_default()
}

fn run(app: AppHandle) {
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
        let ordinal = now.date_naive().num_days_from_ce();
        let action = decide(
            ordinal,
            now.hour(),
            settings.monitor.scheduled_hour,
            settings.monitor.scheduled_catchup_hours,
            settings.monitor.scheduled_last_run.as_deref(),
        );

        let day_key = match &action {
            SchedAction::Wait => continue,
            SchedAction::RunToday { day_key }
            | SchedAction::RunMissed { day_key }
            | SchedAction::SkipMissed { day_key } => day_key.clone(),
        };

        // Stamp before running: a crash during a multi-minute scan must not
        // make the next wake-up start a second unattended cleanup.
        let stamped = day_key.clone();
        services.store.update_settings(|settings| {
            settings.monitor.scheduled_last_run = Some(stamped);
        });
        let _ = services.store.flush_if_dirty();

        if matches!(action, SchedAction::RunToday { .. } | SchedAction::RunMissed { .. }) {
            if let Err(error) = run_daily(&app) {
                eprintln!("scheduled cleanup failed: {error}");
            }
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
            return Err("err.scanTimedOut".into());
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

    #[test]
    fn fires_in_the_scheduled_hour_once_per_day() {
        // 2026-03-10 is ordinal 739_138 (2026-01-01 = 739_140 - 69).
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .num_days_from_ce();
        assert_eq!(
            decide(day, 4, 4, 12, None),
            SchedAction::RunToday {
                day_key: "2026-69".into()
            }
        );
        assert_eq!(
            decide(day, 4, 4, 12, Some("2026-69")),
            SchedAction::Wait,
            "a second wake in the same hour must not fire again"
        );
    }

    #[test]
    fn missed_slot_is_made_up_inside_the_catchup_window() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .num_days_from_ce();
        // App starts at 09:00; slot was 04:00, 5 hours ago, window 12h.
        assert_eq!(
            decide(day, 9, 4, 12, Some("2026-68")),
            SchedAction::RunMissed {
                day_key: "2026-69".into()
            }
        );
        // Outside the window: skip but stamp, so it is never reconsidered.
        assert_eq!(
            decide(day, 20, 4, 12, Some("2026-68")),
            SchedAction::SkipMissed {
                day_key: "2026-69".into()
            }
        );
        // Catch-up disabled: the missed slot is simply skipped.
        assert_eq!(
            decide(day, 9, 4, 0, Some("2026-68")),
            SchedAction::SkipMissed {
                day_key: "2026-69".into()
            }
        );
    }

    #[test]
    fn before_the_slot_it_waits() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .num_days_from_ce();
        assert_eq!(decide(day, 2, 4, 12, None), SchedAction::Wait);
    }
}
