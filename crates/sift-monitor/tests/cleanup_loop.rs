//! The whole cleanup loop, end to end.
//!
//! This is the test that proves the three analysis requirements compose:
//!
//! 1. **scan** a real tree ([`sift_scan`]);
//! 2. **analyze** it and let an adjudicator decide deletability, safety and
//!    reason ([`sift_analyze`]);
//! 3. **persist** the definitively-cleanable conclusions locally and reload them
//!    ([`sift_store`]);
//! 4. **monitor** free space and act on the approved conclusions when the
//!    threshold is crossed ([`sift_monitor`]), with the actual removal injected
//!    so the loop is provable in CI rather than dependent on the platform trash.
//!
//! It also walks the loop four times on four different days to show that the
//! persisted decisions become a routine suggestion — the "basis for later
//! analysis" the product promises.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sift_analyze::{
    AnalysisPolicy, AnalysisReport, Analyzer, RuleAdjudicator, Safety,
};
use sift_core::{ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest};
use sift_monitor::{
    clean_now, evaluate, DiskSample, MonitorConfig, Remover,
};
use sift_platform::trash::TrashResult;
use sift_scan::ScanEngine;
use sift_store::{AutoCleanMode, MonitorSettings, Store, StorePaths};

const KB: u64 = 1024;
const DAY_MS: i64 = 86_400_000;
const GB: u64 = 1024 * 1024 * 1024;

/// A remover that records what it was asked to delete and then actually deletes
/// it, so downstream assertions can see the entry is gone.
#[derive(Default)]
struct RecordingRemover {
    asked: Mutex<Vec<PathBuf>>,
    refuse: Mutex<Vec<PathBuf>>,
}

impl RecordingRemover {
    fn asked(&self) -> Vec<PathBuf> {
        self.asked.lock().unwrap().clone()
    }

    /// Make one path fail, to prove per-item failure handling.
    fn refuse(&self, path: PathBuf) {
        self.refuse.lock().unwrap().push(path);
    }
}

impl Remover for RecordingRemover {
    fn remove(&self, paths: &[PathBuf]) -> Vec<TrashResult> {
        self.asked.lock().unwrap().extend(paths.iter().cloned());
        let refused = self.refuse.lock().unwrap().clone();
        paths
            .iter()
            .map(|path| {
                if refused.contains(path) {
                    return TrashResult::failure(path.clone(), "locked");
                }
                let removed = std::fs::remove_dir_all(path)
                    .or_else(|_| std::fs::remove_file(path))
                    .is_ok();
                if removed {
                    TrashResult::success(path.clone())
                } else {
                    TrashResult::failure(path.clone(), "could not remove")
                }
            })
            .collect()
    }
}

fn fixture(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("sift-loop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("project/node_modules/dep")).unwrap();
    std::fs::create_dir_all(base.join("project/build")).unwrap();
    std::fs::create_dir_all(base.join("Downloads")).unwrap();
    // Real bytes so the aggregate is real.
    std::fs::write(base.join("project/node_modules/dep/lib.bin"), vec![7u8; 256 * KB as usize]).unwrap();
    std::fs::write(base.join("project/build/output.bin"), vec![8u8; 128 * KB as usize]).unwrap();
    std::fs::write(base.join("Downloads/Installer.dmg"), vec![9u8; 64 * KB as usize]).unwrap();
    base
}

/// Scan `root` and wait for the tree to settle.
///
/// Returns the handle rather than the tree: the tree is owned by the handle's
/// lock (and is far too large to clone casually), so callers borrow it.
fn scan(root: &Path) -> (sift_scan::ScanHandle, ScanOutcome) {
    let engine = ScanEngine::with_workers(2);
    let handle = engine
        .scan(ScanRequest::new(ScanId(1), root).with_policy(ScanPolicy::thorough()))
        .expect("scan starts");
    let mut outcome = ScanOutcome::Failed;
    while let Ok(event) = handle.events.recv() {
        if let ScanEvent::Finished { outcome: finished, .. } = event {
            outcome = finished;
            break;
        }
    }
    handle.join();
    (handle, outcome)
}

/// Analyze the scanned tree under its lock.
fn analyze_handle(handle: &sift_scan::ScanHandle, base: &Path) -> AnalysisReport {
    let mut tree = handle.tree.lock().unwrap();
    analyze(&mut tree, base)
}

fn analyze(tree: &mut sift_core::ScanTree, base: &Path) -> AnalysisReport {
    // A low floor so the small fixtures are candidates; the file pass covers
    // the Downloads directory.
    let policy = AnalysisPolicy::default()
        .with_min_bytes(4 * KB)
        .with_search_dirs(vec![base.join("Downloads")]);
    let analyzer = Analyzer::new(policy, None);
    analyzer.analyze(&RuleAdjudicator::new(), tree, 0, |_, _, _| {})
}

fn store_at(tag: &str) -> (Store, StorePaths) {
    let root = std::env::temp_dir().join(format!("sift-loop-store-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = StorePaths::at(root);
    let (store, warnings) = Store::open(paths.clone());
    assert!(warnings.is_empty(), "clean start expected: {warnings:?}");
    (store, paths)
}

#[test]
fn scan_analyze_persist_then_monitor_and_clean() {
    let base = fixture("full");
    let (handle, outcome) = scan(&base);
    assert_eq!(outcome, ScanOutcome::Completed);

    // ---- 1. analysis decides safety and reason ----------------------------
    let report = analyze_handle(&handle, &base);
    let by_name = |name: &str| {
        report
            .items
            .iter()
            .find(|item| item.candidate.name == name)
            .unwrap_or_else(|| panic!("{name} should be a candidate: {:?}", names(&report)))
    };
    let modules = by_name("node_modules");
    assert_eq!(modules.verdict.safety, Safety::Safe, "caches are safe");
    assert!(
        modules.verdict.reason.describe().contains("reason."),
        "a local verdict explains itself with a translation key"
    );
    let build = by_name("build");
    assert_eq!(build.verdict.safety, Safety::Review, "project dirs need a human");
    // The file pass found the installer and treated it as a decision.
    let installer = by_name("Installer.dmg");
    assert_eq!(installer.verdict.safety, Safety::Review);

    // ---- 2. conclusions persist, and only the safe ones are remembered -----
    let (store, paths) = store_at("full");
    let summary = store.record_analysis(&report, 1_000);
    assert_eq!(summary.inserted, 1, "only node_modules is definitively cleanable");
    assert_eq!(summary.not_cleanable, 2);
    assert_eq!(store.cleanable_len(), 1);
    assert!(store.lookup_verdict(&modules.candidate.fingerprint(), 0).is_some());

    // Nothing is auto-eligible until the user approves it.
    let config = MonitorConfig::from_settings(&MonitorSettings {
        auto_mode: AutoCleanMode::AutoApproved,
        ..MonitorSettings::default()
    });
    let entries = store.cleanable().entries().to_vec();
    let at_critical = DiskSample::new(1000 * GB, 10 * GB, 0);
    let before_approval = evaluate(&config, &at_critical, &entries, Some(&base), None, None, 0);
    assert!(!before_approval.deletes(), "approval is required");

    // The user approves the structural findings.
    assert_eq!(store.approve_all_structural(), 1);
    store.save().unwrap();

    // ---- 3. a restart keeps the conclusions -------------------------------
    drop(store);
    let (store, warnings) = Store::open(paths.clone());
    assert!(warnings.is_empty());
    assert_eq!(store.cleanable_len(), 1, "the cleanable list survived a restart");
    let entry = store.cleanable().entries()[0].clone();
    assert_eq!(entry.name, "node_modules");
    assert!(entry.approved_for_auto, "approval survived too");

    // ---- 4. the monitor decides, the remover acts ------------------------
    let entries = store.cleanable().entries().to_vec();
    let decision = evaluate(&config, &at_critical, &entries, Some(&base), None, None, 0);
    assert!(decision.deletes(), "a critical disk with approved entries acts");
    assert_eq!(decision.selected().len(), 1);
    assert_eq!(decision.selected()[0].name, "node_modules");

    let remover = RecordingRemover::default();
    let outcome = clean_now(
        decision.selected(),
        Some(&base),
        true,
        &store,
        &remover,
        2_000,
    );
    assert_eq!(outcome.removed, 1, "the approved entry was removed");
    assert_eq!(outcome.skipped, 0);
    assert_eq!(outcome.failed, 0);
    assert!(outcome.bytes > 0);
    assert_eq!(remover.asked().len(), 1);
    assert!(!base.join("project/node_modules").exists(), "gone from disk");
    assert!(base.join("project/build").exists(), "review items are untouched");

    // The store learned from it: the entry left the list, a decision remains.
    assert_eq!(store.cleanable_len(), 0);
    assert_eq!(store.decisions().len(), 1);
    store.save().unwrap();

    // ---- 5. an unapproved or out-of-home entry is never removed -----------
    let fresh = fixture("guards");
    let (handle2, _) = scan(&fresh);
    let report2 = analyze_handle(&handle2, &fresh);
    let (store2, _) = store_at("guards");
    store2.record_analysis(&report2, 0);
    // Approve, then move the home boundary away from the entry.
    store2.approve_all_structural();
    let entries2 = store2.cleanable().entries().to_vec();
    let elsewhere = clean_now(entries2.as_slice(), Some(Path::new("/somewhere/else")), true, &store2, &RecordingRemover::default(), 0);
    assert_eq!(elsewhere.removed, 0);
    assert_eq!(elsewhere.skipped, 1, "outside home is skipped, not deleted");
    assert!(fresh.join("project/node_modules").exists());

    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&fresh);
    let _ = std::fs::remove_dir_all(paths.root());
}

#[test]
fn a_failing_removal_is_reported_per_item_and_kept_in_the_list() {
    let base = fixture("failure");
    let (handle, _) = scan(&base);
    let report = analyze_handle(&handle, &base);
    let (store, paths) = store_at("failure");
    store.record_analysis(&report, 0);
    store.approve_all_structural();

    let target = base.join("project/node_modules");
    let remover = RecordingRemover::default();
    remover.refuse(target.clone());

    let entries = store.cleanable().entries().to_vec();
    let outcome = clean_now(&entries, Some(&base), true, &store, &remover, 0);

    assert_eq!(outcome.removed, 0);
    assert_eq!(outcome.failed, 1, "the platform refusal is reported");
    assert!(target.exists(), "a failed removal leaves the tree intact");
    assert_eq!(
        store.cleanable_len(),
        1,
        "an entry that was not removed stays remembered"
    );

    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(paths.root());
}

/// Four separate days, the same conclusion: the log turns into a routine.
#[test]
fn repeated_cleanups_become_a_routine_suggestion() {
    let base = fixture("habits");
    let (store, paths) = store_at("habits");

    for day in 0..4i64 {
        let now = day * DAY_MS;
        // Recreate the cache each day: cleaning it is the recurring act.
        std::fs::create_dir_all(base.join("project/node_modules/dep")).unwrap();
        std::fs::write(
            base.join("project/node_modules/dep/lib.bin"),
            vec![7u8; 256 * KB as usize],
        )
        .unwrap();

        let (handle, outcome) = scan(&base);
        assert_eq!(outcome, ScanOutcome::Completed);
        let report = analyze_handle(&handle, &base);
        store.record_analysis(&report, now);
        store.approve_all_structural();

        let entries = store.cleanable().entries().to_vec();
        let outcome = clean_now(
            &entries,
            Some(&base),
            true,
            &store,
            &RecordingRemover::default(),
            now,
        );
        assert_eq!(outcome.removed, 1, "day {day}");
    }

    // Four removals on four days is a habit worth offering as a routine.
    let suggestions = store.decisions().suggest_routines(3, 3);
    assert_eq!(suggestions.len(), 1, "one habit: {:?}", suggestions);
    assert_eq!(suggestions[0].name, "node_modules");
    assert_eq!(suggestions[0].occurrences, 4);
    assert_eq!(suggestions[0].distinct_days, 4);
    assert!(
        suggestions[0].is_destructive(),
        "a repeated removal suggests cleaning, not keeping"
    );

    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(paths.root());
}

/// The router is the privacy boundary: structural families never reach a model.
#[test]
fn only_content_dependent_families_are_adjudicable_by_a_model() {
    let base = fixture("privacy");
    let (handle, _) = scan(&base);
    let report = analyze_handle(&handle, &base);

    let modules = report
        .items
        .iter()
        .find(|item| item.candidate.name == "node_modules")
        .unwrap();
    let installer = report
        .items
        .iter()
        .find(|item| item.candidate.name == "Installer.dmg")
        .unwrap();

    assert!(
        !sift_analyze::RoutingAdjudicator::<RuleAdjudicator, NoRemote>::is_transmittable(
            &modules.candidate
        ),
        "a rebuildable cache is resolved locally"
    );
    assert!(
        sift_analyze::RoutingAdjudicator::<RuleAdjudicator, NoRemote>::is_transmittable(
            &installer.candidate
        ),
        "an installer is content-dependent and may be reviewed"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// A remote adjudicator that is never called in this test; it exists to name the
/// router's type parameters.
struct NoRemote;

impl sift_analyze::RemoteAdjudicator for NoRemote {
    fn provider(&self) -> &str {
        "none"
    }

    fn adjudicate_remote(
        &self,
        _batch: &[sift_analyze::Candidate],
        _guardrails: &sift_analyze::Guardrails,
        _now_ms: i64,
    ) -> Result<Vec<sift_analyze::Verdict>, sift_analyze::AdjudicateError> {
        unreachable!("the router must not transmit structural families")
    }
}

fn names(report: &AnalysisReport) -> Vec<String> {
    report
        .items
        .iter()
        .map(|item| item.candidate.name.clone())
        .collect()
}
