//! The store facade: one place that owns every piece of durable local state.
//!
//! Callers get a single `Store` and never think about files. Internally there is
//! one lock, because the four documents describe one consistent picture — a
//! verdict, the cleanable list derived from it, and the decision that resolved it
//! should not be able to drift apart mid-update.
//!
//! Persistence is explicit ([`Store::save`]) and also best-effort on drop, so a
//! crash cannot lose a session's conclusions but an unexpected exit cannot leave
//! a half-written file either (writes are atomic — see [`crate::atomic`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use sift_analyze::{
    AnalysisReport, AnalyzedItem, Decision, DecisionLog, DecisionOutcome, PathFingerprint, Verdict,
    VerdictCache,
};
use sift_core::NodeKey;

use crate::atomic::{read_document, write_document};
use crate::cleanable::{CleanableEntry, CleanableList, RecordSummary, DEFAULT_CLEANABLE_CAP};
use crate::history::{CleanupHistory, HistoryEntry, DEFAULT_HISTORY_CAP};
use crate::paths::StorePaths;
use crate::routines::{RoutineEntry, RoutineList, DEFAULT_ROUTINE_CAP};
use crate::settings::Settings;
use crate::verdicts::{VerdictRecord, VerdictTable, DEFAULT_VERDICT_CAP};

/// Default ceiling on the decision log: enough to mine habits, not enough to
/// grow without bound.
pub const DEFAULT_DECISION_CAP: usize = 5_000;

struct Inner {
    verdicts: VerdictTable,
    cleanable: CleanableList,
    decisions: DecisionLog,
    settings: Settings,
    history: CleanupHistory,
    routines: RoutineList,
}

/// One place for every piece of durable local state.
pub struct Store {
    paths: Option<StorePaths>,
    inner: Mutex<Inner>,
    dirty: AtomicBool,
    decision_cap: usize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().unwrap();
        f.debug_struct("Store")
            .field("root", &self.paths.as_ref().map(|paths| paths.root()))
            .field("verdicts", &inner.verdicts.len())
            .field("cleanable", &inner.cleanable.len())
            .field("decisions", &inner.decisions.len())
            .field("dirty", &self.dirty.load(Ordering::Relaxed))
            .finish()
    }
}

impl Store {
    /// Open the store at the platform's data directory.
    ///
    /// Returns warnings instead of failing: unusable documents are archived and
    /// replaced with defaults, and the caller decides how to surface that.
    pub fn open_default() -> (Self, Vec<crate::atomic::StoreWarning>) {
        match StorePaths::discover() {
            Some(paths) => Self::open(paths),
            None => (Self::in_memory(), Vec::new()),
        }
    }

    /// Open the store at `paths`, loading whatever is usable.
    pub fn open(paths: StorePaths) -> (Self, Vec<crate::atomic::StoreWarning>) {
        let mut warnings = Vec::new();

        let verdicts = match read_document::<Vec<VerdictRecord>>(&paths.verdicts()) {
            crate::atomic::LoadOutcome::Loaded(records) => {
                VerdictTable::from_records(records, DEFAULT_VERDICT_CAP)
            }
            other => {
                if let Some(warning) = other.warning("store.what.verdicts") {
                    warnings.push(warning);
                }
                VerdictTable::default()
            }
        };

        let cleanable = match read_document::<Vec<CleanableEntry>>(&paths.cleanable()) {
            crate::atomic::LoadOutcome::Loaded(entries) => {
                CleanableList::from_entries(entries, DEFAULT_CLEANABLE_CAP)
            }
            other => {
                if let Some(warning) = other.warning("store.what.cleanable") {
                    warnings.push(warning);
                }
                CleanableList::default()
            }
        };

        let decisions = match read_document::<DecisionLog>(&paths.decisions()) {
            crate::atomic::LoadOutcome::Loaded(log) => log,
            other => {
                if let Some(warning) = other.warning("store.what.decisions") {
                    warnings.push(warning);
                }
                DecisionLog::new()
            }
        };

        let settings = match read_document::<Settings>(&paths.settings()) {
            crate::atomic::LoadOutcome::Loaded(settings) => settings.normalized(),
            other => {
                if let Some(warning) = other.warning("store.what.settings") {
                    warnings.push(warning);
                }
                Settings::default()
            }
        };

        let history = match read_document::<Vec<HistoryEntry>>(&paths.history()) {
            crate::atomic::LoadOutcome::Loaded(entries) => {
                CleanupHistory::from_entries(entries, DEFAULT_HISTORY_CAP)
            }
            other => {
                if let Some(warning) = other.warning("store.what.history") {
                    warnings.push(warning);
                }
                CleanupHistory::with_cap(DEFAULT_HISTORY_CAP)
            }
        };

        let routines = match read_document::<crate::routines::RoutineDoc>(&paths.routines()) {
            crate::atomic::LoadOutcome::Loaded(doc) => {
                RoutineList::from_doc(doc, DEFAULT_ROUTINE_CAP)
            }
            other => {
                if let Some(warning) = other.warning("store.what.routines") {
                    warnings.push(warning);
                }
                RoutineList::with_cap(DEFAULT_ROUTINE_CAP)
            }
        };

        (
            Self {
                paths: Some(paths),
                inner: Mutex::new(Inner {
                    verdicts,
                    cleanable,
                    decisions,
                    settings,
                    history,
                    routines,
                }),
                dirty: AtomicBool::new(false),
                decision_cap: DEFAULT_DECISION_CAP,
            },
            warnings,
        )
    }

    /// A store that keeps everything in memory. Used by tests, by `--no-config`
    /// runs, and when the platform gives us nowhere to write.
    pub fn in_memory() -> Self {
        Self {
            paths: None,
            inner: Mutex::new(Inner {
                verdicts: VerdictTable::default(),
                cleanable: CleanableList::default(),
                decisions: DecisionLog::new(),
                settings: Settings::default(),
                history: CleanupHistory::with_cap(DEFAULT_HISTORY_CAP),
                routines: RoutineList::with_cap(DEFAULT_ROUTINE_CAP),
            }),
            dirty: AtomicBool::new(false),
            decision_cap: DEFAULT_DECISION_CAP,
        }
    }

    /// Whether changes are waiting to be written.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    pub fn root(&self) -> Option<&std::path::Path> {
        self.paths.as_ref().map(|paths| paths.root())
    }

    // ---- settings ----------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }

    /// Mutate settings and mark the store dirty.
    pub fn update_settings(&self, update: impl FnOnce(&mut Settings)) {
        {
            let mut inner = self.inner.lock().unwrap();
            update(&mut inner.settings);
            inner.settings = inner.settings.clone().normalized();
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    // ---- verdict cache -----------------------------------------------------

    pub fn lookup_verdict(&self, fingerprint: &PathFingerprint, now_ms: i64) -> Option<Verdict> {
        self.inner
            .lock()
            .unwrap()
            .verdicts
            .lookup(fingerprint, now_ms)
    }

    pub fn store_verdict(&self, fingerprint: &PathFingerprint, verdict: &Verdict, now_ms: i64) {
        self.inner
            .lock()
            .unwrap()
            .verdicts
            .store(*fingerprint, verdict, now_ms);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn verdict_count(&self) -> usize {
        self.inner.lock().unwrap().verdicts.len()
    }

    // ---- conclusions -------------------------------------------------------

    /// Fold a whole analysis run into the store.
    ///
    /// Every item goes into the verdict cache (so the next run is cheap); only
    /// `Safe` items enter the cleanable list. This is the method that implements
    /// "persist what is definitively cleanable".
    pub fn record_analysis(&self, report: &AnalysisReport, now_ms: i64) -> RecordSummary {
        let summary = {
            let mut inner = self.inner.lock().unwrap();
            for item in &report.items {
                inner
                    .verdicts
                    .store(item.candidate.fingerprint(), &item.verdict, now_ms);
            }
            inner.cleanable.record_report(report, now_ms)
        };
        self.dirty.store(true, Ordering::Relaxed);
        summary
    }

    /// The known-cleanable list, cloned for the caller.
    pub fn cleanable(&self) -> CleanableList {
        self.inner.lock().unwrap().cleanable.clone()
    }

    pub fn cleanable_len(&self) -> usize {
        self.inner.lock().unwrap().cleanable.len()
    }

    /// Grant or revoke unattended-cleanup approval for one remembered entry.
    pub fn set_auto_approval(&self, key: NodeKey, approved: bool) -> bool {
        let changed = self
            .inner
            .lock()
            .unwrap()
            .cleanable
            .set_auto_approval(key, approved);
        if changed {
            self.dirty.store(true, Ordering::Relaxed);
        }
        changed
    }

    /// Approve every structural (non-model) conclusion for unattended cleanup.
    pub fn approve_all_structural(&self) -> usize {
        let approved = self
            .inner
            .lock()
            .unwrap()
            .cleanable
            .approve_all_structural();
        if approved > 0 {
            self.dirty.store(true, Ordering::Relaxed);
        }
        approved
    }

    /// Forget one remembered entry.
    pub fn forget_cleanable(&self, key: NodeKey) -> bool {
        let removed = self.inner.lock().unwrap().cleanable.remove(key).is_some();
        if removed {
            self.dirty.store(true, Ordering::Relaxed);
        }
        removed
    }

    /// Drop remembered entries whose path is gone, preferring a live key set.
    pub fn prune(&self, live: Option<&std::collections::HashSet<NodeKey>>) -> usize {
        let pruned = {
            let mut inner = self.inner.lock().unwrap();
            let pruned = inner.cleanable.prune(live);
            if let Some(live) = live {
                inner.verdicts.retain_keys(live);
            }
            pruned
        };
        if pruned > 0 {
            self.dirty.store(true, Ordering::Relaxed);
        }
        pruned
    }

    // ---- decisions ---------------------------------------------------------

    /// Record the user's decision. This is the raw material for habit mining.
    pub fn record_decision(&self, decision: Decision) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.decisions.record(decision);
            if inner.decisions.len() > self.decision_cap {
                let excess = inner.decisions.len() - self.decision_cap;
                inner.decisions.entries.drain(..excess);
            }
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Record that an entry was successfully sent to the trash: it leaves the
    /// cleanable list and enters the decision log, which is what lets the
    /// product learn that this is something the user removes regularly.
    pub fn note_removed(&self, entry: &CleanableEntry, now_ms: i64) {
        let decision = Decision::new(
            entry.fingerprint,
            entry.display_path.clone(),
            entry.name.clone(),
            entry.kind_token.clone(),
            entry.size,
            DecisionOutcome::Removed,
            now_ms,
        );
        {
            let mut inner = self.inner.lock().unwrap();
            inner.cleanable.remove(entry.key());
            inner.decisions.record(decision);
            if inner.decisions.len() > self.decision_cap {
                let excess = inner.decisions.len() - self.decision_cap;
                inner.decisions.entries.drain(..excess);
            }
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Record that the user explicitly kept something, which suppresses future
    /// suggestions.
    pub fn note_kept(&self, item: &AnalyzedItem, now_ms: i64) {
        let decision = Decision::new(
            item.candidate.fingerprint(),
            item.candidate.display_path.clone(),
            item.candidate.name.clone(),
            item.candidate.kind.token().to_string(),
            item.candidate.reclaimable(),
            DecisionOutcome::Kept,
            now_ms,
        );
        self.record_decision(decision);
    }

    pub fn decisions(&self) -> DecisionLog {
        self.inner.lock().unwrap().decisions.clone()
    }

    // ---- history ----------------------------------------------------------

    pub fn history(&self) -> CleanupHistory {
        self.inner.lock().unwrap().history.clone()
    }

    /// Append one finished cleanup session to the user-facing timeline.
    pub fn record_history(&self, entry: HistoryEntry) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.history.record(entry);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Record a finished cleanup session from short labels and totals.
    pub fn note_cleanup_session(
        &self,
        titles: &[String],
        bytes: u64,
        automatic: bool,
        now_ms: i64,
    ) {
        let entry = HistoryEntry {
            id: crate::history::new_id(now_ms),
            at_ms: now_ms,
            bytes,
            items: titles.len(),
            automatic,
            titles: titles.to_vec(),
        };
        self.record_history(entry);
    }

    // ---- routines ---------------------------------------------------------

    pub fn routines(&self) -> RoutineList {
        self.inner.lock().unwrap().routines.clone()
    }

    /// Add or replace one saved routine.
    pub fn upsert_routine(&self, entry: RoutineEntry) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.routines.upsert(entry);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Delete one saved routine; returns whether it existed.
    pub fn delete_routine(&self, id: &str) -> bool {
        let removed = {
            let mut inner = self.inner.lock().unwrap();
            inner.routines.remove(id)
        };
        if removed {
            self.dirty.store(true, Ordering::Relaxed);
        }
        removed
    }

    /// Persistently dismiss one mined suggestion.
    pub fn ignore_routine_suggestion(&self, key: String) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.routines.ignore_suggestion(key);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Flip one routine between auto-run and approve-before-run.
    pub fn toggle_routine_mode(&self, id: &str) -> bool {
        let changed = {
            let mut inner = self.inner.lock().unwrap();
            inner.routines.toggle_mode(id)
        };
        if changed {
            self.dirty.store(true, Ordering::Relaxed);
        }
        changed
    }

    // ---- persistence -------------------------------------------------------

    /// Write every document. Atomic per file, so a partial failure leaves the
    /// earlier files updated rather than corrupt.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(paths) = &self.paths else {
            self.dirty.store(false, Ordering::Relaxed);
            return Ok(());
        };
        let (verdicts, cleanable, decisions, settings) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.verdicts.records().to_vec(),
                inner.cleanable.entries().to_vec(),
                inner.decisions.clone(),
                inner.settings.clone(),
            )
        };
        let (history, routines) = {
            let inner = self.inner.lock().unwrap();
            (inner.history.entries().to_vec(), inner.routines.to_doc())
        };

        write_document(&paths.verdicts(), &verdicts)?;
        write_document(&paths.cleanable(), &cleanable)?;
        write_document(&paths.decisions(), &decisions)?;
        write_document(&paths.settings(), &settings)?;
        write_document(&paths.history(), &history)?;
        write_document(&paths.routines(), &routines)?;

        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Save only when something changed. Returns whether a write happened.
    pub fn flush_if_dirty(&self) -> std::io::Result<bool> {
        if !self.is_dirty() {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// Where the store writes, when it persists at all.
    pub fn paths(&self) -> Option<&StorePaths> {
        self.paths.as_ref()
    }

    /// A path for diagnostics and the "reveal in Finder" affordance.
    pub fn root_display(&self) -> String {
        self.paths
            .as_ref()
            .map(|paths| paths.root().display().to_string())
            .unwrap_or_else(|| "<memory>".to_string())
    }
}

/// The store is the product's [`VerdictCache`], so the analysis pipeline can
/// reuse conclusions without knowing anything about files.
impl VerdictCache for Store {
    fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
        self.lookup_verdict(fingerprint, 0)
    }

    fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict) {
        self.store_verdict(fingerprint, verdict, verdict.judged_at_ms);
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Best effort: losing conclusions on an unexpected exit is worse than a
        // write that fails silently during teardown.
        if self.is_dirty() {
            let _ = self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::{Candidate, CandidateKind, Evidence, Reason, Safety, VerdictSource};
    use sift_core::ByteSize;
    use std::path::{Path, PathBuf};

    fn temp_paths(tag: &str) -> StorePaths {
        let root = std::env::temp_dir().join(format!("sift-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        StorePaths::at(root)
    }

    fn item(name: &str, safety: Safety, size: u64) -> AnalyzedItem {
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(name.as_bytes()),
            PathBuf::from("/Users/me/project").join(name),
            format!("~/project/{name}"),
            name,
            true,
            ByteSize::new(size, size),
            0,
            CandidateKind::RebuildableCache { tool: "npm".into() },
        );
        candidate
            .evidence
            .push(Evidence::new("dir.node_modules", "detail.x"));
        AnalyzedItem::new(
            candidate,
            Verdict::new(
                safety,
                0.9,
                Reason::key("reason.rebuildableCache"),
                VerdictSource::rule("dir.node_modules"),
                1,
            ),
        )
    }

    fn report(items: Vec<AnalyzedItem>) -> AnalysisReport {
        AnalysisReport::new(items, 0, 1)
    }

    #[test]
    fn an_in_memory_store_needs_no_platform_directories() {
        let store = Store::in_memory();
        assert!(store.root().is_none());
        assert_eq!(store.root_display(), "<memory>");
        assert!(
            store.save().is_ok(),
            "saving in memory is a no-op, not a failure"
        );
    }

    #[test]
    fn conclusions_survive_a_restart() {
        let paths = temp_paths("restart");
        let (store, warnings) = Store::open(paths.clone());
        assert!(warnings.is_empty());

        let summary = store.record_analysis(
            &report(vec![
                item("node_modules", Safety::Safe, 500),
                item("build", Safety::Review, 700),
            ]),
            100,
        );
        assert_eq!(summary.inserted, 1);
        assert_eq!(summary.not_cleanable, 1);
        store.save().unwrap();
        drop(store);

        let (reopened, warnings) = Store::open(paths.clone());
        assert!(warnings.is_empty());
        assert_eq!(reopened.cleanable_len(), 1, "only the safe conclusion");
        assert_eq!(reopened.verdict_count(), 2, "both verdicts are cached");
        let cleanable = reopened.cleanable();
        assert_eq!(cleanable.entries()[0].name, "node_modules");
        assert_eq!(cleanable.entries()[0].size, 500);
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn the_verdict_cache_answers_the_analysis_pipeline() {
        let store = Store::in_memory();
        let analyzed = item("node_modules", Safety::Safe, 500);
        let fingerprint = analyzed.candidate.fingerprint();

        assert!(VerdictCache::lookup(&store, &fingerprint).is_none());
        VerdictCache::store(&store, &fingerprint, &analyzed.verdict);
        let found = VerdictCache::lookup(&store, &fingerprint).expect("cached");
        assert_eq!(found.safety, Safety::Safe);
    }

    #[test]
    fn approval_round_trips_and_prunes_with_removal() {
        let store = Store::in_memory();
        store.record_analysis(&report(vec![item("node_modules", Safety::Safe, 500)]), 1);
        let key = store.cleanable().entries()[0].key();
        assert!(store.set_auto_approval(key, true));
        assert!(store.cleanable().entries()[0].approved_for_auto);

        let entry = store.cleanable().entries()[0].clone();
        store.note_removed(&entry, 2);
        assert_eq!(store.cleanable_len(), 0);
        assert_eq!(store.decisions().len(), 1);
        assert_eq!(
            store.decisions().entries[0].outcome,
            DecisionOutcome::Removed
        );
    }

    #[test]
    fn habits_can_be_mined_from_the_persisted_log() {
        let paths = temp_paths("habits");
        let (store, _) = Store::open(paths.clone());
        for day in 0..4 {
            let analyzed = item("node_modules", Safety::Safe, 500);
            store.record_analysis(&report(vec![analyzed.clone()]), day * 86_400_000);
            let mut entry = store.cleanable().entries()[0].clone();
            entry.first_seen_ms = day * 86_400_000;
            store.note_removed(&entry, day * 86_400_000);
        }
        let suggestions = store.decisions().suggest_routines(3, 3);
        assert_eq!(suggestions.len(), 1, "four separate days is a habit");
        assert_eq!(suggestions[0].name, "node_modules");
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn settings_persist_and_are_normalized_on_load() {
        let paths = temp_paths("settings");
        let (store, _) = Store::open(paths.clone());
        store.update_settings(|settings| {
            settings.monitor.warn_free_ratio = 0.25;
            settings.monitor.critical_free_ratio = 0.9; // invalid: above warn
            settings.monitor.auto_mode = crate::settings::AutoCleanMode::AutoApproved;
        });
        store.save().unwrap();
        drop(store);

        let (reopened, _) = Store::open(paths.clone());
        let settings = reopened.settings();
        assert_eq!(settings.monitor.warn_free_ratio, 0.25);
        assert!(settings.monitor.critical_free_ratio < 0.25);
        assert!(settings.monitor.auto_mode.may_delete());
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn a_corrupt_document_is_archived_and_reported_without_losing_the_rest() {
        let paths = temp_paths("corrupt");
        let (store, _) = Store::open(paths.clone());
        store.record_analysis(&report(vec![item("node_modules", Safety::Safe, 500)]), 1);
        store.save().unwrap();

        // Corrupt only the cleanable list.
        std::fs::write(paths.cleanable(), b"{ broken").unwrap();

        let (reopened, warnings) = Store::open(paths.clone());
        assert_eq!(warnings.len(), 1, "the failure must be reported");
        assert_eq!(warnings[0].what, "store.what.cleanable");
        assert!(reopened.cleanable().is_empty());
        // The verdict cache is untouched, so the next analysis still benefits.
        assert_eq!(reopened.verdict_count(), 1);
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn dirty_tracking_drives_flush() {
        let store = Store::in_memory();
        assert!(!store.is_dirty());
        assert!(!store.flush_if_dirty().unwrap());
        store.record_analysis(&report(vec![item("x", Safety::Safe, 1)]), 1);
        assert!(store.is_dirty());
        assert!(store.flush_if_dirty().unwrap());
        assert!(!store.is_dirty());
    }

    #[test]
    fn pruning_uses_the_live_key_set() {
        let store = Store::in_memory();
        store.record_analysis(
            &report(vec![
                item("alive", Safety::Safe, 1),
                item("dead", Safety::Safe, 2),
            ]),
            1,
        );
        let live: std::collections::HashSet<NodeKey> =
            [NodeKey::from_bytes(b"alive")].into_iter().collect();
        assert_eq!(store.prune(Some(&live)), 1);
        assert_eq!(store.cleanable_len(), 1);
    }

    #[test]
    fn explicit_keeps_are_recorded_for_habit_mining() {
        let store = Store::in_memory();
        let analyzed = item("build", Safety::Review, 700);
        store.note_kept(&analyzed, 1);
        let log = store.decisions();
        assert_eq!(log.len(), 1);
        assert_eq!(log.entries[0].outcome, DecisionOutcome::Kept);
    }

    #[test]
    fn drop_flushes_pending_changes() {
        let paths = temp_paths("drop");
        {
            let (store, _) = Store::open(paths.clone());
            store.record_analysis(&report(vec![item("node_modules", Safety::Safe, 9)]), 1);
            // No explicit save: dropping must persist.
        }
        let (reopened, _) = Store::open(paths.clone());
        assert_eq!(reopened.cleanable_len(), 1);
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn debug_output_summarises_without_leaking_paths() {
        let store = Store::in_memory();
        store.record_analysis(&report(vec![item("node_modules", Safety::Safe, 1)]), 1);
        let text = format!("{store:?}");
        assert!(text.contains("cleanable: 1"));
        assert!(
            !text.contains("/Users/me"),
            "debug output must not leak paths"
        );
        let _ = Path::new("/");
    }
}
