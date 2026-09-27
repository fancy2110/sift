//! The persisted verdict cache.
//!
//! Reusing a judgment is what makes the second analysis of the same volume
//! nearly free and, more importantly, what keeps the product's conclusions
//! *stable*: the same cache directory is not re-litigated — or re-transmitted
//! to a model — on every scan.
//!
//! A record is only reused when the whole [`PathFingerprint`] matches, so a
//! verdict about a 4 GB cache cannot authorize deleting whatever replaces it.
//! The table is bounded and evicts the least recently used entries, because an
//! unbounded cache of every path ever seen is exactly the memory growth this
//! project is trying to avoid on disk rather than in RAM.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sift_analyze::{PathFingerprint, Verdict};
use sift_core::NodeKey;

/// One cached judgment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictRecord {
    pub fingerprint: PathFingerprint,
    pub verdict: Verdict,
    /// When this record was last matched, for LRU eviction.
    pub last_used_ms: i64,
    /// How many times it has been reused.
    pub times_used: u32,
}

/// Default ceiling: comfortably above the number of items a user ever looks at,
/// comfortably below "the whole filesystem".
pub const DEFAULT_VERDICT_CAP: usize = 20_000;

/// A bounded, indexed table of cached verdicts.
#[derive(Debug, Clone)]
pub struct VerdictTable {
    records: Vec<VerdictRecord>,
    /// `NodeKey` to the indices of its records. One path can hold several
    /// records when its size or mtime changed between judgments.
    index: HashMap<NodeKey, Vec<usize>>,
    cap: usize,
}

impl Default for VerdictTable {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_VERDICT_CAP)
    }
}

impl VerdictTable {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            records: Vec::new(),
            index: HashMap::new(),
            cap: cap.max(16),
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Look up a verdict for exactly this fingerprint.
    pub fn lookup(&self, fingerprint: &PathFingerprint, now_ms: i64) -> Option<Verdict> {
        let indices = self.index.get(&fingerprint.key)?;
        for &index in indices {
            let record = &self.records[index];
            if record.fingerprint.still_describes(fingerprint) {
                return Some(record.verdict.clone());
            }
        }
        let _ = now_ms;
        None
    }

    /// Look up without mutating (the read-only path).
    pub fn peek(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
        self.lookup(fingerprint, 0)
    }

    /// Record the use of an existing match, for LRU accounting.
    pub fn touch(&mut self, fingerprint: &PathFingerprint, now_ms: i64) {
        if let Some(indices) = self.index.get(&fingerprint.key) {
            for &index in indices {
                let record = &mut self.records[index];
                if record.fingerprint.still_describes(fingerprint) {
                    record.last_used_ms = now_ms;
                    record.times_used = record.times_used.saturating_add(1);
                    return;
                }
            }
        }
    }

    /// Insert or refresh a verdict.
    ///
    /// A new fingerprint for the same path *replaces* the old record rather than
    /// accumulating: the old judgment described a different object and keeping
    /// it would only consume the budget.
    pub fn store(&mut self, fingerprint: PathFingerprint, verdict: &Verdict, now_ms: i64) {
        if let Some(indices) = self.index.get_mut(&fingerprint.key) {
            for &index in indices.iter() {
                if self.records[index]
                    .fingerprint
                    .still_describes(&fingerprint)
                {
                    let record = &mut self.records[index];
                    record.verdict = verdict.clone();
                    record.last_used_ms = now_ms;
                    record.times_used = record.times_used.saturating_add(1);
                    return;
                }
            }
            // Same path, different content: drop the stale record.
            let stale: Vec<usize> = indices.clone();
            self.remove_indices(&stale);
        }

        let index = self.records.len();
        self.records.push(VerdictRecord {
            fingerprint,
            verdict: verdict.clone(),
            last_used_ms: now_ms,
            times_used: 1,
        });
        self.index.entry(fingerprint.key).or_default().push(index);

        if self.records.len() > self.cap {
            self.evict_to_cap();
        }
    }

    /// Drop records for paths that no longer exist, given the set of keys that
    /// were seen in the latest scan.
    ///
    /// Without this, a volume that is rescanned after heavy churn would keep
    /// verdicts for thousands of deleted files.
    pub fn retain_keys(&mut self, live: &std::collections::HashSet<NodeKey>) {
        let dead: Vec<usize> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| !live.contains(&record.fingerprint.key))
            .map(|(index, _)| index)
            .collect();
        self.remove_indices(&dead);
    }

    /// Evict least-recently-used records until the table fits its cap.
    pub fn evict_to_cap(&mut self) {
        if self.records.len() <= self.cap {
            return;
        }
        let excess = self.records.len() - self.cap;
        let mut order: Vec<usize> = (0..self.records.len()).collect();
        order.sort_by_key(|&index| self.records[index].last_used_ms);
        let victims: Vec<usize> = order.into_iter().take(excess).collect();
        self.remove_indices(&victims);
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.index.clear();
    }

    /// Everything held, for serialization.
    pub fn records(&self) -> &[VerdictRecord] {
        &self.records
    }

    /// Rebuild from a loaded list, dropping duplicates and re-applying the cap.
    pub fn from_records(records: Vec<VerdictRecord>, cap: usize) -> Self {
        let mut table = Self::with_capacity(cap);
        for record in records {
            table.store(record.fingerprint, &record.verdict, record.last_used_ms);
        }
        table
    }

    fn remove_indices(&mut self, victims: &[usize]) {
        if victims.is_empty() {
            return;
        }
        let victim_set: std::collections::HashSet<usize> = victims.iter().copied().collect();
        let mut keep: Vec<VerdictRecord> = Vec::with_capacity(self.records.len() - victims.len());
        for (index, record) in self.records.drain(..).enumerate() {
            if !victim_set.contains(&index) {
                keep.push(record);
            }
        }
        self.records = keep;
        self.reindex();
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (index, record) in self.records.iter().enumerate() {
            self.index
                .entry(record.fingerprint.key)
                .or_default()
                .push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::{Reason, Safety, VerdictSource};

    fn verdict(safety: Safety, confidence: f32) -> Verdict {
        Verdict::new(
            safety,
            confidence,
            Reason::key("k"),
            VerdictSource::rule("dir.node_modules"),
            0,
        )
    }

    fn fingerprint(name: &str, size: u64, mtime: i64) -> PathFingerprint {
        PathFingerprint::new(NodeKey::from_bytes(name.as_bytes()), size, mtime, true)
    }

    #[test]
    fn stores_and_reuses_an_exact_match() {
        let mut table = VerdictTable::default();
        let fp = fingerprint("a", 100, 5);
        table.store(fp, &verdict(Safety::Safe, 0.9), 10);
        let found = table.lookup(&fp, 20).expect("hit");
        assert_eq!(found.safety, Safety::Safe);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_changed_file_invalidates_its_verdict() {
        let mut table = VerdictTable::default();
        table.store(fingerprint("a", 100, 5), &verdict(Safety::Safe, 0.9), 10);
        assert!(table.lookup(&fingerprint("a", 101, 5), 0).is_none(), "size");
        assert!(
            table.lookup(&fingerprint("a", 100, 6), 0).is_none(),
            "mtime"
        );
        assert!(table.lookup(&fingerprint("b", 100, 5), 0).is_none(), "path");
    }

    #[test]
    fn a_new_fingerprint_replaces_the_old_record_for_the_same_path() {
        let mut table = VerdictTable::default();
        table.store(fingerprint("a", 100, 5), &verdict(Safety::Safe, 0.9), 10);
        table.store(fingerprint("a", 200, 9), &verdict(Safety::Review, 0.5), 11);
        assert_eq!(table.len(), 1, "one path keeps one record");
        assert!(table.lookup(&fingerprint("a", 100, 5), 0).is_none());
        assert!(table.lookup(&fingerprint("a", 200, 9), 0).is_some());
    }

    #[test]
    fn touching_records_use_for_lru() {
        let mut table = VerdictTable::default();
        let fp = fingerprint("a", 1, 1);
        table.store(fp, &verdict(Safety::Safe, 0.9), 100);
        table.touch(&fp, 500);
        assert_eq!(table.records()[0].last_used_ms, 500);
        assert_eq!(table.records()[0].times_used, 2);
    }

    #[test]
    fn the_table_evicts_the_least_recently_used() {
        let mut table = VerdictTable::with_capacity(16);
        for index in 0..20u64 {
            table.store(
                fingerprint(&format!("p{index}"), index, 0),
                &verdict(Safety::Safe, 0.9),
                index as i64,
            );
        }
        assert!(table.len() <= 16, "cap must hold, got {}", table.len());
        // The oldest entries are gone; the newest survive.
        assert!(table.lookup(&fingerprint("p0", 0, 0), 0).is_none());
        assert!(table.lookup(&fingerprint("p19", 19, 0), 0).is_some());
    }

    #[test]
    fn dead_paths_can_be_pruned() {
        let mut table = VerdictTable::default();
        table.store(fingerprint("alive", 1, 1), &verdict(Safety::Safe, 0.9), 0);
        table.store(fingerprint("dead", 1, 1), &verdict(Safety::Safe, 0.9), 0);

        let live: std::collections::HashSet<NodeKey> =
            [NodeKey::from_bytes(b"alive")].into_iter().collect();
        table.retain_keys(&live);
        assert_eq!(table.len(), 1);
        assert!(table.lookup(&fingerprint("alive", 1, 1), 0).is_some());
        assert!(table.lookup(&fingerprint("dead", 1, 1), 0).is_none());
    }

    #[test]
    fn records_round_trip_through_the_table() {
        let mut table = VerdictTable::default();
        table.store(fingerprint("a", 1, 1), &verdict(Safety::Safe, 0.9), 7);
        table.store(fingerprint("b", 2, 2), &verdict(Safety::Review, 0.4), 8);

        let restored = VerdictTable::from_records(table.records().to_vec(), DEFAULT_VERDICT_CAP);
        assert_eq!(restored.len(), 2);
        assert_eq!(
            restored.lookup(&fingerprint("a", 1, 1), 0).unwrap().safety,
            Safety::Safe
        );
        assert_eq!(
            restored.lookup(&fingerprint("b", 2, 2), 0).unwrap().safety,
            Safety::Review
        );
    }

    #[test]
    fn json_round_trip_keeps_everything_needed() {
        let mut table = VerdictTable::default();
        table.store(fingerprint("a", 1, 1), &verdict(Safety::Safe, 0.9), 7);
        let json = serde_json::to_string(table.records()).unwrap();
        let records: Vec<VerdictRecord> = serde_json::from_str(&json).unwrap();
        let restored = VerdictTable::from_records(records, DEFAULT_VERDICT_CAP);
        assert_eq!(restored.len(), 1);
        let found = restored.lookup(&fingerprint("a", 1, 1), 0).unwrap();
        assert_eq!(found.safety, Safety::Safe);
        assert_eq!(found.source, VerdictSource::rule("dir.node_modules"));
    }
}
