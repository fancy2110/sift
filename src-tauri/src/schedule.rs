//! Daily scheduled cleanup.
//!
//! A small background thread that wakes a few times a minute and, when the
//! local clock reaches the configured hour (default 04:00), scans the system
//! volume and analyses it. It deliberately **does not delete anything**: the
//! result is a list of cleanable items the user reviews and confirms in the
//! app. Scheduled cleanup is an analysis pass, not an unattended deletion.

use std::path::Path;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use sift_analyze::Safety;
use sift_store::CleanableEntry;
use tauri::{AppHandle, Emitter, Manager};

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
        // Make it up once if still inside the catch-up window; otherwise record
        // the day as handled without running an unattended cleanup.
        let hours_after_slot = now_hour - scheduled_hour;
        if catchup_hours > 0 && hours_after_slot <= catchup_hours {
            return SchedAction::RunMissed { day_key };
        }
        return SchedAction::SkipMissed { day_key };
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
        // The scheduled toggle is its own explicit consent. It deliberately
        // does NOT depend on the low-space auto-clean switch: a user who
        // enables "daily cleanup" expects it to run even while the low-space
        // trigger is off (that was the previous silent no-op). Every deletion
        // still has to clear the per-item guards — Safe, individually
        // approved for auto cleanup, inside home, and an unchanged
        // fingerprint — checked by run_daily and perform_cleanup.
        if !settings.monitor.scheduled_cleanup_enabled {
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

/// Run every enabled cleanup rule and return `(matches, removed)`.
/// A rule is an explicit user instruction, so this runs regardless of the
/// findings auto-execute switch.
fn run_enabled_cleanup_rules(
    store: &sift_store::Store,
    rules: &[sift_store::CleanupRule],
) -> (usize, usize) {
    let mut matches = 0usize;
    let mut removed = 0usize;
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if let Ok(preview) = crate::cleanup_rules::preview_cleanup_rule(rule.clone()) {
            matches += preview.matched;
        }
        match crate::cleanup_rules::run_rule(store, rule) {
            Ok(result) => removed += result.removed,
            Err(error) => {
                eprintln!("scheduled cleanup rule \"{}\" failed: {error}", rule.name)
            }
        }
    }
    (matches, removed)
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

    // The switch decides the outcome: off means "analyse and wait", on means
    // "execute the approved cleanup and record it in history".
    let auto_execute = services
        .store
        .settings()
        .monitor
        .scheduled_auto_execute;

    // Scheduled cleanup stops here unless auto-execute is on. Count what
    // the pass found, bring the window forward and let the front end show the
    // smart list for confirmation.
    let home = dirs::home_dir();
    let pending = services
        .store
        .cleanable()
        .entries()
        .iter()
        .filter(|entry| entry.safety == Safety::Safe)
        .filter(|entry| sift_monitor::policy::resolved_inside_home(&entry.path, home.as_deref()))
        .count();

    // An enabled cleanup rule is itself the user's explicit instruction:
    // "clean files matching this rule". Run it on every scheduled pass,
    // independently of the findings auto-execute switch. The preview count is
    // kept for the notification; the rule's own guards still decide what is
    // actually removable.
    let rules = services.store.settings().monitor.cleanup_rules;
    let (rule_matches, _) = run_enabled_cleanup_rules(&services.store, &rules);

    if auto_execute {
        let paths = select_approved_paths(
            services.store.cleanable().entries(),
            home.as_deref(),
        );
        if !paths.is_empty() {
            crate::analyze::perform_cleanup(&services, &paths, true);
        }
        return Ok(());
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit(
        "scheduled://ready",
        ScheduledReady {
            pending,
            rule_matches,
        },
    );
    Ok(())
}

/// Payload for the front end when a scheduled analysis pass finishes.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledReady {
    pending: usize,
    rule_matches: usize,
}

/// Select remembered entries for unattended scheduled cleanup: `Safe`,
/// individually approved for automatic cleanup, and proven to be inside
/// `home`. A missing home proves nothing, so nothing is selected.
///
/// The membership test is the filesystem-proven
/// [`sift_monitor::policy::resolved_inside_home`], not a lexical prefix: a
/// directory inside home that is (or became) a symlink pointing outside must
/// not be eligible for unattended deletion. This mirrors what the monitor's
/// `clean_now` re-checks immediately before a move, so selection and execution
/// share one boundary rule. A path that cannot be resolved counts as "outside"
/// — it would fail the pre-delete fingerprint check anyway.
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
        .filter(|entry| sift_monitor::policy::resolved_inside_home(&entry.path, Some(home)))
        .map(|entry| entry.path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use sift_analyze::{PathFingerprint, Reason, VerdictSource};
    use sift_core::NodeKey;

    /// A throwaway tree: a home with an inside target, and an outside target.
    /// Selection now proves membership on the real filesystem, so the fixtures
    /// must exist.
    struct Fixture {
        root: PathBuf,
        home: PathBuf,
        outside: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("sift-sched-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let home = root.join("home");
            let outside = root.join("outside");
            std::fs::create_dir_all(home.join("project/a")).unwrap();
            std::fs::create_dir_all(home.join("project/b")).unwrap();
            std::fs::create_dir_all(home.join("project/c")).unwrap();
            std::fs::create_dir_all(outside.join("d")).unwrap();
            Self {
                root,
                home,
                outside,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn entry(name: &str, path: impl Into<PathBuf>, safety: Safety, approved: bool) -> CleanableEntry {
        let path = path.into();
        let display = path.to_string_lossy().into_owned();
        CleanableEntry {
            fingerprint: PathFingerprint::new(NodeKey::from_bytes(name.as_bytes()), 1, 0, true),
            path,
            display_path: display,
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
        let fixture = Fixture::new("select");
        let entries = vec![
            entry("a", fixture.home.join("project/a"), Safety::Safe, true),
            entry("b", fixture.home.join("project/b"), Safety::Safe, false),
            entry("c", fixture.home.join("project/c"), Safety::Review, true),
            entry("d", fixture.outside.join("d"), Safety::Safe, true),
        ];
        let paths = select_approved_paths(&entries, Some(&fixture.home));
        assert_eq!(
            paths,
            vec![fixture
                .home
                .join("project/a")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn missing_home_selects_nothing() {
        let fixture = Fixture::new("nohome");
        let entries = vec![entry(
            "a",
            fixture.home.join("project/a"),
            Safety::Safe,
            true,
        )];
        assert!(select_approved_paths(&entries, None).is_empty());
    }

    #[test]
    fn a_vanished_path_is_not_selected() {
        // It cannot be proven inside home (realpath fails) and would fail the
        // pre-delete fingerprint check anyway: refusing is the safe direction.
        let fixture = Fixture::new("gone");
        let entries = vec![entry(
            "gone",
            fixture.home.join("project/deleted-already"),
            Safety::Safe,
            true,
        )];
        assert!(select_approved_paths(&entries, Some(&fixture.home)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_home_that_points_outside_is_not_selected() {
        // Lexically this path is under home; the real filesystem says it lives
        // outside. Unattended deletion must follow the real target.
        let fixture = Fixture::new("symlink");
        let link = fixture.home.join("link");
        std::os::unix::fs::symlink(&fixture.outside, &link).unwrap();

        let entries = vec![
            entry("escaped", link.join("d"), Safety::Safe, true),
            entry(
                "inside",
                fixture.home.join("project/a"),
                Safety::Safe,
                true,
            ),
        ];
        let paths = select_approved_paths(&entries, Some(&fixture.home));
        assert_eq!(
            paths,
            vec![fixture
                .home
                .join("project/a")
                .to_string_lossy()
                .into_owned()],
            "the outside-pointing link is dropped, the real inner path kept"
        );
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
    #[test]
    fn enabled_rules_run_without_the_findings_auto_switch() {
        let dir = std::env::temp_dir().join(format!("sift-rule-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("old.dmg");
        std::fs::write(&file, b"x").unwrap();
        let store = sift_store::Store::in_memory();
        store.update_settings(|settings| {
            settings.monitor.cleanup_method = "delete".into();
            settings.monitor.scheduled_auto_execute = false;
        });
        let rule = sift_store::CleanupRule {
            id: "test".into(),
            name: "test".into(),
            dir: dir.to_string_lossy().into_owned(),
            pattern: r"\.dmg$".into(),
            older_than_days: 0,
            enabled: true,
        };
        let (matched, removed) = run_enabled_cleanup_rules(&store, &[rule]);
        assert_eq!(matched, 1);
        assert_eq!(removed, 1);
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ======================================================================
    // Unified-trigger tests (scheduled-task review, 2026-11).
    //
    // These tests use mock scheduled tasks that live entirely inside test
    // state; no production data or settings are touched. Two layers:
    //
    // Layer 1 — `CleanupRule`: production ALREADY shares one runner. The daily
    // scheduler reaches it through `run_enabled_cleanup_rules` (this module)
    // and the manual command through `cleanup_rules::run_rule`; both call the
    // same `run_rule` function. The tests pin that contract, including every
    // failure path.
    //
    // Layer 2 — routine-shaped mock task: routines have no automatic scheduler
    // in production yet (their cadence is a stored label only). This thin
    // test-local harness builds the target pattern out of production parts —
    // the real clock decision (`decide`) and the real cleanup funnel
    // (`analyze::perform_cleanup`) — and proves an automatic fire and a manual
    // fire can share one entry with identical results and failure handling.
    // ======================================================================

    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use sift_analyze::{AnalysisPolicy, Analyzer, RuleAdjudicator};
    use sift_core::{ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest};
    use sift_scan::ScanEngine;

    fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Remove a scratch tree on drop, so a failing assertion cannot leak it.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("sift-unified-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Build `AppServices` around an in-memory store, exactly as the command
    /// layer would have them, without touching the platform data directory.
    fn test_services(store: Arc<sift_store::Store>) -> crate::analyze::AppServices {
        crate::analyze::AppServices::with_store_for_test(store)
    }

    // ---- Layer 1: cleanup rules already share one runner ------------------

    fn dmg_rule(dir: &Path, enabled: bool) -> sift_store::CleanupRule {
        sift_store::CleanupRule {
            id: "test-rule".into(),
            name: "test rule".into(),
            dir: dir.to_string_lossy().into_owned(),
            pattern: r"\.dmg$".into(),
            older_than_days: 0,
            enabled,
        }
    }

    /// A rule fixture: one matching installer, one non-matching text file,
    /// and a store that deletes permanently inside the temp tree (no Finder).
    fn rule_fixture(tag: &str) -> (TempTree, sift_store::Store) {
        let tree = TempTree::new(tag);
        std::fs::write(tree.path().join("installer.dmg"), b"x").unwrap();
        std::fs::write(tree.path().join("notes.txt"), b"x").unwrap();
        let store = sift_store::Store::in_memory();
        store.update_settings(|settings| {
            settings.monitor.cleanup_method = "delete".into();
        });
        (tree, store)
    }

    #[test]
    fn rule_auto_and_manual_triggers_share_one_runner_and_agree() {
        let (auto_tree, auto_store) = rule_fixture("rule-auto");
        let (manual_tree, manual_store) = rule_fixture("rule-manual");
        let auto_rule = dmg_rule(auto_tree.path(), true);
        let manual_rule = dmg_rule(manual_tree.path(), true);

        // Automatic trigger: the exact helper the daily scheduler calls from
        // `run_daily` (see `run_enabled_cleanup_rules` above).
        let (auto_matched, auto_removed) =
            run_enabled_cleanup_rules(&auto_store, std::slice::from_ref(&auto_rule));

        // Manual trigger: the exact function behind the `run_cleanup_rule`
        // Tauri command (cleanup_rules.rs).
        let manual = crate::cleanup_rules::run_rule(&manual_store, &manual_rule).unwrap();

        // Same runner, same counts, same filesystem outcome.
        assert_eq!(auto_matched, manual.matched);
        assert_eq!(auto_removed, manual.removed);
        assert_eq!((auto_matched, auto_removed), (1, 1));
        assert_eq!(manual.failed, 0);
        for tree in [auto_tree.path(), manual_tree.path()] {
            assert!(!tree.join("installer.dmg").exists(), "matched file removed");
            assert!(tree.join("notes.txt").exists(), "non-matching file kept");
        }

        // Both record one automatic cleanup session: the shared runner always
        // marks rule cleanups `automatic = true`, whoever invoked it.
        let auto_sessions = auto_store.history();
        let auto_history = auto_sessions.entries();
        let manual_sessions = manual_store.history();
        let manual_history = manual_sessions.entries();
        assert_eq!(auto_history.len(), 1);
        assert_eq!(manual_history.len(), 1);
        assert!(auto_history[0].automatic);
        assert!(manual_history[0].automatic);
        assert_eq!(auto_history[0].items, manual_history[0].items);
        assert_eq!(auto_history[0].bytes, manual_history[0].bytes);

        // A disabled rule is invisible to the scheduler (its filter requires
        // `enabled`), so an unattended pass does nothing.
        let (disabled_tree, disabled_store) = rule_fixture("rule-disabled");
        let (matched, removed) = run_enabled_cleanup_rules(
            &disabled_store,
            std::slice::from_ref(&dmg_rule(disabled_tree.path(), false)),
        );
        assert_eq!((matched, removed), (0, 0));
        assert!(disabled_tree.path().join("installer.dmg").exists());
    }

    #[cfg(unix)]
    fn running_as_root() -> bool {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|id| id.trim() == "0")
    }

    #[test]
    fn rule_failures_reach_both_triggers_through_the_same_guardrails() {
        let (tree, store) = rule_fixture("rule-errors");

        // Protected folder: the shared runner refuses before scanning.
        store.update_settings(|settings| {
            settings
                .monitor
                .protected_paths
                .push(tree.path().to_string_lossy().into_owned());
        });
        let protected = dmg_rule(tree.path(), true);
        assert_eq!(
            crate::cleanup_rules::run_rule(&store, &protected).unwrap_err(),
            "err.protectedPath"
        );
        // The scheduler wrapper logs the same error and continues; the
        // preview still counted the match, but nothing was removed. The
        // guardrail decision is identical even though error surfacing differs
        // (command returns `Err`; the unattended loop prints and carries on).
        let (matched, removed) =
            run_enabled_cleanup_rules(&store, std::slice::from_ref(&protected));
        assert_eq!((matched, removed), (1, 0));
        assert!(tree.path().join("installer.dmg").exists());

        // Missing directory and malformed regex are validated on a separate,
        // non-protected tree: the protected prefix above would otherwise
        // answer first. Both fail for the manual command before any deletion.
        let other = TempTree::new("rule-validation");
        let missing = dmg_rule(&other.path().join("does-not-exist"), true);
        assert_eq!(
            crate::cleanup_rules::run_rule(&store, &missing).unwrap_err(),
            "err.ruleDirMissing"
        );

        // Malformed regex: validation fails, never deletes.
        let mut bad_regex = dmg_rule(other.path(), true);
        bad_regex.pattern = "(".into();
        assert!(crate::cleanup_rules::run_rule(&store, &bad_regex)
            .unwrap_err()
            .starts_with("err.ruleRegex|"));
        assert!(tree.path().join("installer.dmg").exists());

        // Per-item removal failure (Unix, non-root): an unwritable parent
        // directory makes the delete fail closed and be reported as one failed
        // item by the shared runner, whichever trigger invoked it.
        #[cfg(unix)]
        if !running_as_root() {
            use std::os::unix::fs::PermissionsExt;

            let locked_tree = TempTree::new("rule-locked");
            let locked_dir = locked_tree.path().join("locked");
            std::fs::create_dir_all(&locked_dir).unwrap();
            std::fs::write(locked_dir.join("locked.dmg"), b"x").unwrap();
            let mut locked_rule = dmg_rule(&locked_dir, true);
            locked_rule.id = "locked-rule".into();
            std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o500))
                .unwrap();

            // Manual trigger: a per-item failure DTO, the fixture survives.
            let dto = crate::cleanup_rules::run_rule(&store, &locked_rule).unwrap();
            assert_eq!(dto.removed, 0);
            assert_eq!(dto.failed, 1);
            assert!(locked_dir.join("locked.dmg").exists());

            // Automatic trigger through the scheduler helper: same outcome.
            let (_, auto_removed) =
                run_enabled_cleanup_rules(&store, std::slice::from_ref(&locked_rule));
            assert_eq!(auto_removed, 0);
            assert!(locked_dir.join("locked.dmg").exists());

            // Restore writability so the Drop guard can clean up.
            std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
    }

    // ---- Layer 2: routine-shaped mock task on a unified harness -----------

    /// Why a mock task fired. The unified runner treats both the same; the
    /// label exists so call sites document the reason and a future production
    /// scheduler has a single place to branch if it ever needs to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TaskTrigger {
        /// The scheduler clock reached the slot.
        AutoScheduled,
        /// The user pressed "run now".
        Manual,
    }

    /// Test stand-in for a unified scheduled task: a routine with a real
    /// schedule and explicit path targets, mirroring the path-based
    /// `RoutineEntry` rows the app already persists.
    struct MockTask {
        id: String,
        scheduled_hour: u32,
        catchup_hours: u32,
        last_run: Option<String>,
        paths: Vec<String>,
    }

    impl MockTask {
        fn path_task(id: &str, paths: Vec<String>) -> Self {
            Self {
                id: id.into(),
                scheduled_hour: 4,
                catchup_hours: 12,
                last_run: None,
                paths,
            }
        }
    }

    /// The scheduler's real clock decision, reused verbatim — no test clock
    /// reimplementation.
    fn task_due(task: &MockTask, ordinal: i32, hour: u32) -> SchedAction {
        decide(
            ordinal,
            hour,
            task.scheduled_hour,
            task.catchup_hours,
            task.last_run.as_deref(),
        )
    }

    /// The ONE execution entry both triggers use.
    ///
    /// It is a thin wrapper over the production funnel
    /// [`crate::analyze::perform_cleanup`], which is itself the synchronous
    /// body shared by the interactive `clean_paths`, the manual
    /// `run_routine`, the tray "clean safe items" and the daily scheduler's
    /// auto-execute branch. `run_routine` reaches the same guardrail function
    /// (`perform_cleanup_with`) via its off-thread wrapper; routine-shaped
    /// tasks therefore need no second deletion path once a scheduler fires
    /// them.
    fn run_mock_task(
        services: &crate::analyze::AppServices,
        task: &MockTask,
        trigger: TaskTrigger,
    ) -> Vec<crate::cleanup::DeleteResultItem> {
        // The trigger deliberately changes nothing about how the task runs.
        match trigger {
            TaskTrigger::AutoScheduled | TaskTrigger::Manual => {}
        }
        // The id is the task's stable identity (the key a production scheduler
        // would stamp per-run state against); assert it is always present.
        debug_assert!(!task.id.is_empty());
        // Mirror `run_routine`'s path-based resolution: targets that vanished
        // are skipped while the stored paths stay intact.
        let paths: Vec<String> = task
            .paths
            .iter()
            .filter(|path| Path::new(path).exists())
            .cloned()
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        // Routine runs are recorded as unattended-style cleanups in production
        // too (`run_routine` passes automatic = true), whatever the trigger.
        crate::analyze::perform_cleanup(services, &paths, true)
    }

    /// Compare results without depending on `DeleteResultItem: PartialEq`.
    fn result_signature(
        results: &[crate::cleanup::DeleteResultItem],
    ) -> Vec<(bool, Option<String>, bool)> {
        results
            .iter()
            .map(|item| (item.ok, item.error.clone(), item.cancelled))
            .collect()
    }

    /// A scanned, analyzed and approved fixture: a Safe rebuildable cache plus
    /// a Review build output, remembered by an in-memory store. The whole
    /// production pipeline runs here; no cleanable entry is fabricated.
    struct AnalyzedFixture {
        root: PathBuf,
        services: crate::analyze::AppServices,
        /// A `Safe`, approved rebuildable cache that unattended cleanup may remove.
        cache: PathBuf,
        /// A `Review` build output that every unattended run must leave alone.
        build: PathBuf,
    }

    impl AnalyzedFixture {
        fn build(tag: &str) -> Self {
            // Owned directly: the fixture's own Drop is the only guard, so the
            // tree outlives the constructor (a nested TempTree would delete it
            // on return).
            let root = std::env::temp_dir()
                .join(format!("sift-unified-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let cache = root.join("project/node_modules");
            std::fs::create_dir_all(cache.join("dep")).unwrap();
            std::fs::write(cache.join("dep/lib.bin"), vec![7u8; 256 * 1024]).unwrap();
            let build = root.join("project/build");
            std::fs::create_dir_all(&build).unwrap();
            std::fs::write(build.join("output.bin"), vec![8u8; 128 * 1024]).unwrap();
            // Keep the non-canonical temp path deliberately: on macOS
            // canonicalization turns `/var/...` into `/private/var/...`, and
            // the deletion policy refuses anything under the protected
            // `private` root (sift-core deletable::is_protected_location).

            // Real scan + rule analysis, following the established
            // sift-monitor integration-test pattern.
            let engine = ScanEngine::with_workers(2);
            let handle = engine
                .scan(
                    ScanRequest::new(ScanId(1), &root).with_policy(ScanPolicy::thorough()),
                )
                .expect("scan starts");
            let mut outcome = ScanOutcome::Failed;
            while let Ok(event) = handle.events.recv() {
                if let ScanEvent::Finished {
                    outcome: finished, ..
                } = event
                {
                    outcome = finished;
                    break;
                }
            }
            handle.join();
            assert_eq!(outcome, ScanOutcome::Completed);

            let policy = AnalysisPolicy::default().with_min_bytes(4 * 1024);
            let analyzer = Analyzer::new(policy, None);
            let report = {
                let mut scan_tree = handle.tree.lock().unwrap();
                analyzer.analyze(
                    &RuleAdjudicator::new(),
                    &mut scan_tree,
                    now_millis(),
                    |_, _, _| {},
                )
            };

            let store = sift_store::Store::in_memory();
            store.update_settings(|settings| {
                // Delete inside the temp tree rather than talking to Finder;
                // every guardrail still runs on this code path.
                settings.monitor.cleanup_method = "delete".into();
            });
            let summary = store.record_analysis(&report, now_millis());
            assert!(summary.inserted >= 1, "the Safe cache must be remembered");
            assert!(
                store.approve_all_structural() >= 1,
                "the structural Safe item must be approvable"
            );

            let services = test_services(Arc::new(store));
            Self {
                root,
                services,
                cache,
                build,
            }
        }
    }

    impl Drop for AnalyzedFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn mock_task_clock_drives_an_automatic_fire_that_matches_the_manual_fire() {
        let auto = AnalyzedFixture::build("mock-auto");
        let manual = AnalyzedFixture::build("mock-manual");
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .num_days_from_ce();

        // ---- automatic trigger: the clock decides, then the task fires ----
        let mut task = MockTask::path_task(
            "mock:cache",
            vec![auto.cache.to_string_lossy().into_owned()],
        );
        assert_eq!(task_due(&task, day, 2), SchedAction::Wait);
        let day_key = match task_due(&task, day, 4) {
            SchedAction::RunToday { day_key } => day_key,
            other => panic!("expected RunToday at the slot, got {other:?}"),
        };
        // The production loop stamps before running so a crash cannot start a
        // second unattended cleanup; the harness keeps the same ordering.
        task.last_run = Some(day_key);
        assert_eq!(
            task_due(&task, day, 4),
            SchedAction::Wait,
            "a second wake in the slot must not fire again"
        );
        let auto_results = run_mock_task(&auto.services, &task, TaskTrigger::AutoScheduled);

        // ---- manual trigger: the clock says Wait, "run now" fires anyway --
        let manual_task = MockTask::path_task(
            "mock:cache",
            vec![manual.cache.to_string_lossy().into_owned()],
        );
        assert_eq!(task_due(&manual_task, day, 2), SchedAction::Wait);
        let manual_results =
            run_mock_task(&manual.services, &manual_task, TaskTrigger::Manual);

        // ---- same funnel, identical outcomes and side effects -------------
        assert_eq!(
            result_signature(&auto_results),
            result_signature(&manual_results)
        );
        assert_eq!(auto_results.len(), 1);
        assert!(auto_results[0].ok && manual_results[0].ok);
        assert!(!auto.cache.exists(), "the automatic fire really removed the cache");
        assert!(!manual.cache.exists(), "the manual fire really removed the cache");
        assert!(auto.build.exists(), "Review items survive unattended cleanup");
        assert!(manual.build.exists());

        for fixture in [&auto, &manual] {
            assert_eq!(fixture.services.store.cleanable_len(), 0);
            let sessions = fixture.services.store.history();
            let history = sessions.entries();
            assert_eq!(history.len(), 1);
            assert!(history[0].automatic);
            assert_eq!(history[0].items, 1);
        }
        assert_eq!(
            auto.services.store.history().entries()[0].bytes,
            manual.services.store.history().entries()[0].bytes
        );
    }

    #[test]
    fn mock_task_honors_catchup_and_never_runs_an_expired_missed_slot() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .num_days_from_ce();

        // Restart at 09:00 for a 04:00 slot, inside the 12-hour window: the
        // missed run is made up exactly once.
        let made_up = AnalyzedFixture::build("mock-catchup");
        let mut task = MockTask {
            id: "mock:catchup".into(),
            scheduled_hour: 4,
            catchup_hours: 12,
            last_run: Some("2026-68".into()),
            paths: vec![made_up.cache.to_string_lossy().into_owned()],
        };
        match task_due(&task, day, 9) {
            SchedAction::RunMissed { day_key } => {
                task.last_run = Some(day_key);
                let results =
                    run_mock_task(&made_up.services, &task, TaskTrigger::AutoScheduled);
                assert_eq!(results.len(), 1);
                assert!(results[0].ok);
            }
            other => panic!("expected RunMissed inside the window, got {other:?}"),
        }
        assert!(!made_up.cache.exists());
        assert_eq!(task_due(&task, day, 9), SchedAction::Wait);

        // Restart at 20:00, outside the window: stamp and skip, never run an
        // unattended cleanup for a slot that expired yesterday.
        let skipped = AnalyzedFixture::build("mock-skip");
        let skip_task = MockTask {
            id: "mock:skip".into(),
            scheduled_hour: 4,
            catchup_hours: 12,
            last_run: Some("2026-68".into()),
            paths: vec![skipped.cache.to_string_lossy().into_owned()],
        };
        match task_due(&skip_task, day, 20) {
            SchedAction::SkipMissed { day_key } => assert_eq!(day_key, "2026-69"),
            other => panic!("expected SkipMissed outside the window, got {other:?}"),
        }
        assert!(
            skipped.cache.exists(),
            "an expired missed slot must never reach the cleanup funnel"
        );
        assert_eq!(skipped.services.store.cleanable_len(), 1);
        assert!(skipped.services.store.history().entries().is_empty());
    }

    #[test]
    fn mock_task_failures_are_handled_identically_by_both_triggers() {
        let auto = AnalyzedFixture::build("mock-fail-auto");
        let manual = AnalyzedFixture::build("mock-fail-manual");

        // (a) Unknown path — never analyzed, never remembered: refused, not
        // silently dropped.
        let auto_ghost = auto.root.join("never-seen.bin");
        std::fs::write(&auto_ghost, b"x").unwrap();
        let manual_ghost = manual.root.join("never-seen.bin");
        std::fs::write(&manual_ghost, b"x").unwrap();
        let ghost_task = |path: &Path| MockTask::path_task("mock:ghost", vec![path.to_string_lossy().into_owned()]);
        let auto_ghost_results =
            run_mock_task(&auto.services, &ghost_task(&auto_ghost), TaskTrigger::AutoScheduled);
        let manual_ghost_results =
            run_mock_task(&manual.services, &ghost_task(&manual_ghost), TaskTrigger::Manual);
        assert_eq!(
            result_signature(&auto_ghost_results),
            result_signature(&manual_ghost_results)
        );
        assert_eq!(auto_ghost_results.len(), 1);
        assert!(!auto_ghost_results[0].ok);
        assert_eq!(
            auto_ghost_results[0].error.as_deref(),
            Some("err.deleteNotConfirmed")
        );
        assert!(auto_ghost.exists() && manual_ghost.exists());

        // (b) The approved object was replaced by something else after
        // approval: the fingerprint no longer describes it, so both triggers
        // refuse rather than delete an object nobody approved.
        std::fs::remove_dir_all(&auto.cache).unwrap();
        std::fs::write(&auto.cache, b"replaced by a file").unwrap();
        std::fs::remove_dir_all(&manual.cache).unwrap();
        std::fs::write(&manual.cache, b"replaced by a file").unwrap();
        let cache_task =
            |path: &Path| MockTask::path_task("mock:cache", vec![path.to_string_lossy().into_owned()]);
        let auto_changed =
            run_mock_task(&auto.services, &cache_task(&auto.cache), TaskTrigger::AutoScheduled);
        let manual_changed =
            run_mock_task(&manual.services, &cache_task(&manual.cache), TaskTrigger::Manual);
        assert_eq!(
            result_signature(&auto_changed),
            result_signature(&manual_changed)
        );
        assert_eq!(auto_changed.len(), 1);
        assert!(!auto_changed[0].ok);
        assert_eq!(
            auto_changed[0].error.as_deref(),
            Some("err.fingerprintChanged")
        );
        assert!(auto.cache.exists() && manual.cache.exists());

        // (c) Every target vanished: the task resolves to an empty list and
        // returns success-without-work on both paths.
        let empty_task = MockTask::path_task(
            "mock:empty",
            vec![auto.root.join("gone").to_string_lossy().into_owned()],
        );
        assert!(run_mock_task(&auto.services, &empty_task, TaskTrigger::AutoScheduled).is_empty());
        assert!(run_mock_task(&manual.services, &empty_task, TaskTrigger::Manual).is_empty());
    }

}
