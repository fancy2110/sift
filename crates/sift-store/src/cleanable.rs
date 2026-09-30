//! The known-cleanable list: the durable answer to "what may we delete?".
//!
//! This file is the product's memory of what it has concluded is *definitively*
//! safe to remove. It is what makes the second run faster than the first, what
//! the background monitor acts on, and what the user reviews when they want to
//! know what the app has decided on their behalf.
//!
//! Two deliberate restrictions keep it trustworthy:
//!
//! * **Only `Safe` entries are stored.** A `Review` item is a question for a
//!   human, not a standing permission, so it lives in the verdict cache (which
//!   records what was asked) and never in this list (which records what may be
//!   done). When a re-analysis downgrades an entry, it leaves this list and the
//!   change is reported.
//! * **Approval is per entry and survives re-analysis.** The user's "yes, always
//!   clean this" is not lost when the same cache is seen again — but an entry
//!   that stops being `Safe` loses its approval, because the approval was given
//!   for a conclusion that no longer holds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sift_analyze::cleanup::{display_command, plan_for};
use sift_analyze::{
    AnalysisReport, AnalyzedItem, ConfidencePolicy, PathFingerprint, Reason, Safety, VerdictSource,
};
use sift_core::NodeKey;

/// Default ceiling on remembered cleanable items.
pub const DEFAULT_CLEANABLE_CAP: usize = 5_000;

/// One remembered conclusion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanableEntry {
    pub fingerprint: PathFingerprint,
    /// The real path. Local file only — this list is never transmitted.
    pub path: PathBuf,
    pub display_path: String,
    pub name: String,
    /// The candidate family token, e.g. `rebuildableCache`.
    pub kind_token: String,
    pub size: u64,
    pub safety: Safety,
    pub confidence: f32,
    pub reason: Reason,
    pub source: VerdictSource,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// How many analyses have re-confirmed this entry.
    pub times_seen: u32,
    /// The user explicitly allowed this entry to be cleaned without asking.
    pub approved_for_auto: bool,
    /// Preferred toolchain-native command, when one exists.
    pub cleanup_command: Option<String>,
    /// Cleanup method token (nativeCommand | trashItem | emptyTrash).
    pub cleanup_method: String,
    /// What breaks after deletion: an i18n key for local entries, ready text
    /// for a model's verdict.
    pub impact: Reason,
}

impl CleanableEntry {
    pub fn key(&self) -> NodeKey {
        self.fingerprint.key
    }

    /// Whether an unattended cleanup may act on this entry.
    ///
    /// Requires three independent things: the conclusion is `Safe`, the entry
    /// still matches the fingerprint it was approved under, and the confidence
    /// clears the bar for its source.
    pub fn is_auto_eligible(&self, confidence: &ConfidencePolicy) -> bool {
        self.approved_for_auto
            && self.safety.is_automatic()
            && self.confidence >= confidence.minimum_for_source(&self.source)
    }

    /// Whether this entry's own conclusion still deserves its place in the list.
    pub fn still_cleanable(&self) -> bool {
        self.safety.is_automatic()
    }
}

/// What an [`CleanableList::upsert`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Upsert {
    /// A new entry was remembered.
    Inserted,
    /// An existing entry was re-confirmed.
    Refreshed,
    /// An existing entry stopped being definitively cleanable and was removed.
    Removed,
    /// The verdict is not `Safe`, so the list does not track it.
    NotCleanable,
}

/// Summary of folding a whole report into the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordSummary {
    pub inserted: usize,
    pub refreshed: usize,
    pub removed: usize,
    /// Items that were not `Safe` and are therefore only in the verdict cache.
    pub not_cleanable: usize,
    /// Bytes currently remembered as cleanable.
    pub cleanable_bytes: u64,
}

/// The bounded list of definitively cleanable entries.
#[derive(Debug, Clone)]
pub struct CleanableList {
    entries: Vec<CleanableEntry>,
    cap: usize,
}

impl Default for CleanableList {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CLEANABLE_CAP)
    }
}

impl CleanableList {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            cap: cap.max(16),
        }
    }

    pub fn entries(&self) -> &[CleanableEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: NodeKey) -> Option<&CleanableEntry> {
        self.entries.iter().find(|entry| entry.key() == key)
    }

    pub fn get_mut(&mut self, key: NodeKey) -> Option<&mut CleanableEntry> {
        self.entries.iter_mut().find(|entry| entry.key() == key)
    }

    /// Total bytes currently remembered as cleanable.
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    /// Entries an unattended cleanup may act on under `confidence`.
    pub fn auto_eligible(&self, confidence: &ConfidencePolicy) -> Vec<&CleanableEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.is_auto_eligible(confidence))
            .collect()
    }

    /// Bytes the monitor could reclaim without asking.
    pub fn auto_eligible_bytes(&self, confidence: &ConfidencePolicy) -> u64 {
        self.auto_eligible(confidence)
            .iter()
            .map(|entry| entry.size)
            .sum()
    }

    /// Insert or refresh one conclusion.
    ///
    /// The user's approval is carried over on refresh, but is revoked when the
    /// conclusion weakens — approval was given for a conclusion, not for a path
    /// forever.
    pub fn upsert(&mut self, item: &AnalyzedItem, now_ms: i64) -> Upsert {
        let key = item.candidate.key;
        let is_cleanable = item.verdict.safety.is_automatic();

        match self.entries.iter().position(|entry| entry.key() == key) {
            Some(index) => {
                if !is_cleanable {
                    self.entries.remove(index);
                    return Upsert::Removed;
                }
                let entry = &mut self.entries[index];
                let was_same_object = entry
                    .fingerprint
                    .still_describes(&item.candidate.fingerprint());
                // An approval belongs to the exact object it was granted for.
                if !was_same_object {
                    entry.approved_for_auto = false;
                }
                entry.fingerprint = item.candidate.fingerprint();
                entry.path = item.candidate.path.clone();
                entry.display_path = item.candidate.display_path.clone();
                entry.name = item.candidate.name.clone();
                entry.kind_token = item.candidate.kind.token().to_string();
                entry.size = item.candidate.reclaimable();
                entry.safety = item.verdict.safety;
                entry.confidence = item.verdict.confidence;
                entry.reason = item.verdict.reason.clone();
                entry.source = item.verdict.source.clone();
                let (cmd, method) = plan_fields(item);
                entry.cleanup_command = cmd;
                entry.cleanup_method = method;
                entry.impact = impact_for(item);
                entry.last_seen_ms = now_ms;
                entry.times_seen = entry.times_seen.saturating_add(1);
                Upsert::Refreshed
            }
            None => {
                if !is_cleanable {
                    return Upsert::NotCleanable;
                }
                self.entries.push(CleanableEntry {
                    fingerprint: item.candidate.fingerprint(),
                    path: item.candidate.path.clone(),
                    display_path: item.candidate.display_path.clone(),
                    name: item.candidate.name.clone(),
                    kind_token: item.candidate.kind.token().to_string(),
                    size: item.candidate.reclaimable(),
                    safety: item.verdict.safety,
                    confidence: item.verdict.confidence,
                    reason: item.verdict.reason.clone(),
                    source: item.verdict.source.clone(),
                    first_seen_ms: now_ms,
                    last_seen_ms: now_ms,
                    times_seen: 1,
                    approved_for_auto: false,
                    cleanup_command: plan_fields(item).0,
                    cleanup_method: plan_fields(item).1,
                    impact: impact_for(item),
                });
                if self.entries.len() > self.cap {
                    self.evict_to_cap();
                }
                Upsert::Inserted
            }
        }
    }

    /// Fold a whole analysis report in, returning what changed.
    pub fn record_report(&mut self, report: &AnalysisReport, now_ms: i64) -> RecordSummary {
        let mut summary = RecordSummary::default();
        for item in &report.items {
            match self.upsert(item, now_ms) {
                Upsert::Inserted => summary.inserted += 1,
                Upsert::Refreshed => summary.refreshed += 1,
                Upsert::Removed => summary.removed += 1,
                Upsert::NotCleanable => summary.not_cleanable += 1,
            }
        }
        summary.cleanable_bytes = self.total_bytes();
        summary
    }

    /// Grant or revoke unattended-cleanup approval for one entry.
    ///
    /// Refuses to approve anything that is not currently definitively
    /// cleanable: an approval must not be able to outlive its conclusion.
    pub fn set_auto_approval(&mut self, key: NodeKey, approved: bool) -> bool {
        match self.get_mut(key) {
            Some(entry) if approved && !entry.still_cleanable() => false,
            Some(entry) => {
                entry.approved_for_auto = approved;
                true
            }
            None => false,
        }
    }

    /// Approve every entry whose conclusion came from a structural rule.
    ///
    /// This is the convenient switch behind "clean the obvious things
    /// automatically" — still opt-in, and still limited to entries the rule
    /// table called `Safe`.
    pub fn approve_all_structural(&mut self) -> usize {
        let mut approved = 0;
        for entry in &mut self.entries {
            if entry.still_cleanable() && !entry.source.is_model() {
                entry.approved_for_auto = true;
                approved += 1;
            }
        }
        approved
    }

    /// Forget an entry, e.g. after it was removed or the user dismissed it.
    pub fn remove(&mut self, key: NodeKey) -> Option<CleanableEntry> {
        let index = self.entries.iter().position(|entry| entry.key() == key)?;
        Some(self.entries.remove(index))
    }

    /// Drop entries whose path no longer exists.
    ///
    /// Prefers a live key set when the caller has one (free, after a scan) and
    /// falls back to a stat per entry, which is bounded by the cap.
    pub fn prune(&mut self, live: Option<&std::collections::HashSet<NodeKey>>) -> usize {
        let before = self.entries.len();
        match live {
            Some(live) => self.entries.retain(|entry| live.contains(&entry.key())),
            None => self.entries.retain(|entry| path_exists(&entry.path)),
        }
        before - self.entries.len()
    }

    /// Keep the largest entries when over budget: they are the ones that matter
    /// to the user and to the monitor's threshold.
    pub fn evict_to_cap(&mut self) {
        if self.entries.len() <= self.cap {
            return;
        }
        self.entries.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.key().cmp(&right.key()))
        });
        self.entries.truncate(self.cap);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn from_entries(entries: Vec<CleanableEntry>, cap: usize) -> Self {
        let mut list = Self::with_capacity(cap);
        list.entries = entries;
        // A loaded file may predate a policy change; re-apply it.
        list.entries.retain(|entry| entry.still_cleanable());
        list.evict_to_cap();
        list
    }
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}


/// Compute the persisted cleanup command and method for an analyzed item.
fn plan_fields(item: &AnalyzedItem) -> (Option<String>, String) {
    let plan = plan_for(&item.candidate.kind, &item.candidate.display_path);
    let command = plan.command.as_ref().map(|step| display_command(step));
    (command, plan.method.token().to_string())
}

/// The deletion impact carried by a cleanable entry: a model's text wins, else
/// the cleanup plan's deterministic i18n key.
fn impact_for(item: &AnalyzedItem) -> Reason {
    item.verdict
        .impact
        .clone()
        .unwrap_or_else(|| Reason::key(plan_for(&item.candidate.kind, &item.candidate.display_path).impact_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::{Candidate, CandidateKind, Evidence};
    use sift_core::ByteSize;

    fn item(
        name: &str,
        kind: CandidateKind,
        rule: &str,
        safety: Safety,
        size: u64,
    ) -> AnalyzedItem {
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(name.as_bytes()),
            PathBuf::from("/Users/me/project").join(name),
            format!("~/project/{name}"),
            name,
            matches!(kind, CandidateKind::RebuildableCache { .. }),
            ByteSize::new(size, size),
            0,
            kind,
        );
        candidate.evidence.push(Evidence::new(rule, "detail.x"));
        let verdict = sift_analyze::adjudicate::rule_verdict(&candidate, 0);
        // The rule decides; if the test wants a different level, override it.
        let verdict = if verdict.safety == safety {
            verdict
        } else {
            sift_analyze::Verdict::new(safety, 0.9, Reason::key("k"), VerdictSource::rule(rule), 0, None)
        };
        AnalyzedItem::new(candidate, verdict)
    }

    fn cache_item(name: &str, size: u64) -> AnalyzedItem {
        item(
            name,
            CandidateKind::RebuildableCache { tool: "npm".into() },
            "dir.node_modules",
            Safety::Safe,
            size,
        )
    }

    #[test]
    fn only_safe_conclusions_are_remembered() {
        let mut list = CleanableList::default();
        assert_eq!(
            list.upsert(&cache_item("node_modules", 100), 0),
            Upsert::Inserted
        );
        let review = item(
            "build",
            CandidateKind::RebuildableCache {
                tool: "build".into(),
            },
            "dir.rust_gradle_build",
            Safety::Review,
            200,
        );
        assert_eq!(list.upsert(&review, 0), Upsert::NotCleanable);
        assert_eq!(list.len(), 1, "review items are questions, not permissions");
        assert_eq!(list.total_bytes(), 100);
    }

    #[test]
    fn refresh_keeps_a_single_row_and_counts_sightings() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 10);
        assert_eq!(
            list.upsert(&cache_item("node_modules", 100), 20),
            Upsert::Refreshed
        );
        assert_eq!(list.len(), 1);
        let entry = &list.entries()[0];
        assert_eq!(entry.times_seen, 2);
        assert_eq!(entry.first_seen_ms, 10);
        assert_eq!(entry.last_seen_ms, 20);
    }

    #[test]
    fn a_downgrade_removes_the_entry() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 0);
        let downgraded = item(
            "node_modules",
            CandidateKind::RebuildableCache { tool: "npm".into() },
            "dir.node_modules",
            Safety::Review,
            100,
        );
        assert_eq!(list.upsert(&downgraded, 5), Upsert::Removed);
        assert!(list.is_empty());
    }

    #[test]
    fn approval_persists_across_reconfirmation() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 0);
        let key = list.entries()[0].key();
        assert!(list.set_auto_approval(key, true));
        assert!(list.entries()[0].approved_for_auto);

        list.upsert(&cache_item("node_modules", 100), 50);
        assert!(
            list.entries()[0].approved_for_auto,
            "re-seeing the same cache must not revoke approval"
        );
    }

    #[test]
    fn approval_is_revoked_when_the_object_changes() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 0);
        let key = list.entries()[0].key();
        list.set_auto_approval(key, true);

        // The same path, now a different size: a different object.
        list.upsert(&cache_item("node_modules", 999), 50);
        assert!(
            !list.entries()[0].approved_for_auto,
            "approval must not transfer to a different object"
        );
    }

    #[test]
    fn nothing_is_auto_eligible_without_approval() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 0);
        let policy = ConfidencePolicy::default();
        assert!(list.auto_eligible(&policy).is_empty());
        assert_eq!(list.auto_eligible_bytes(&policy), 0);

        let key = list.entries()[0].key();
        list.set_auto_approval(key, true);
        assert_eq!(list.auto_eligible(&policy).len(), 1);
        assert_eq!(list.auto_eligible_bytes(&policy), 100);
    }

    #[test]
    fn approving_a_review_entry_is_refused() {
        let mut list = CleanableList::default();
        // Force a Review entry in to prove the guard, bypassing upsert's filter.
        let review = item(
            "build",
            CandidateKind::RebuildableCache {
                tool: "build".into(),
            },
            "dir.rust_gradle_build",
            Safety::Review,
            100,
        );
        list.entries.push(CleanableEntry {
            fingerprint: review.candidate.fingerprint(),
            path: review.candidate.path.clone(),
            display_path: review.candidate.display_path.clone(),
            name: review.candidate.name.clone(),
            kind_token: review.candidate.kind.token().to_string(),
            size: 100,
            safety: Safety::Review,
            confidence: 0.5,
            reason: Reason::key("k"),
            source: VerdictSource::rule("x"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: false,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: Reason::key("cleanup.impact.rebuildRegenerates"),
        });
        let key = list.entries()[0].key();
        assert!(!list.set_auto_approval(key, true));
        assert!(!list.entries()[0].approved_for_auto);
    }

    #[test]
    fn structural_approval_skips_model_conclusions() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 0);

        let mut model_item = cache_item("DerivedData", 300);
        model_item.verdict.source = VerdictSource::remote("test");
        list.upsert(&model_item, 0);

        let approved = list.approve_all_structural();
        assert_eq!(approved, 1, "the rule-sourced entry, not the model's");
        let model_entry = list
            .entries()
            .iter()
            .find(|entry| entry.name == "DerivedData")
            .unwrap();
        assert!(!model_entry.approved_for_auto);
    }

    #[test]
    fn report_recording_summarises_changes() {
        let mut list = CleanableList::default();
        let report = AnalysisReport::new(
            vec![
                cache_item("node_modules", 100),
                cache_item("DerivedData", 200),
                item(
                    "build",
                    CandidateKind::RebuildableCache {
                        tool: "build".into(),
                    },
                    "dir.rust_gradle_build",
                    Safety::Review,
                    400,
                ),
            ],
            0,
            1,
        );
        let summary = list.record_report(&report, 5);
        assert_eq!(summary.inserted, 2);
        assert_eq!(summary.not_cleanable, 1);
        assert_eq!(summary.cleanable_bytes, 300);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn pruning_removes_dead_paths_by_key_set() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("alive", 100), 0);
        list.upsert(&cache_item("dead", 200), 0);
        let live: std::collections::HashSet<NodeKey> =
            [NodeKey::from_bytes(b"alive")].into_iter().collect();
        assert_eq!(list.prune(Some(&live)), 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list.entries()[0].name, "alive");
    }

    #[test]
    fn pruning_by_stat_removes_nonexistent_paths() {
        let base = std::env::temp_dir().join(format!("sift-cleanable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();

        let mut list = CleanableList::default();
        let mut present = cache_item("present", 100);
        present.candidate.path = real.clone();
        list.upsert(&present, 0);
        list.upsert(&cache_item("gone", 100), 0);

        assert_eq!(list.prune(None), 1);
        assert_eq!(list.entries()[0].name, "present");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_cap_keeps_the_largest_entries() {
        let mut list = CleanableList::with_capacity(16);
        for index in 0..24u64 {
            list.upsert(&cache_item(&format!("cache{index}"), index * 100), 0);
        }
        assert!(list.len() <= 16);
        // The largest survive; the smallest are gone.
        assert!(list.get(NodeKey::from_bytes(b"cache23")).is_some());
        assert!(list.get(NodeKey::from_bytes(b"cache0")).is_none());
    }

    #[test]
    fn loading_reapplies_the_cleanable_rule() {
        let entry = CleanableEntry {
            fingerprint: PathFingerprint::new(NodeKey::from_bytes(b"x"), 1, 0, true),
            path: PathBuf::from("/x"),
            display_path: "~/x".into(),
            name: "x".into(),
            kind_token: "trash".into(),
            size: 1,
            safety: Safety::Review,
            confidence: 0.5,
            reason: Reason::key("k"),
            source: VerdictSource::rule("r"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: true,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: Reason::key("k"),
        };
        let list = CleanableList::from_entries(vec![entry], 16);
        assert!(list.is_empty(), "a review entry must not survive loading");
    }

    #[test]
    fn entries_json_round_trip() {
        let mut list = CleanableList::default();
        list.upsert(&cache_item("node_modules", 100), 7);
        let json = serde_json::to_vec(list.entries()).unwrap();
        let back: Vec<CleanableEntry> = serde_json::from_slice(&json).unwrap();
        let restored = CleanableList::from_entries(back, DEFAULT_CLEANABLE_CAP);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.entries()[0].name, "node_modules");
        assert_eq!(restored.entries()[0].first_seen_ms, 7);
    }
}
