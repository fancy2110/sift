//! Cleanup history: one record per finished cleanup session.
//!
//! Unlike the decision log (one row per decided path, mined for habits), the
//! history is the user-facing timeline: each interactive or unattended cleanup
//! becomes one session with an item count, freed bytes and short labels. It is
//! what the home and dashboard surfaces read for "累计释放".

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Build a unique session id (timestamp plus a process-local sequence).
pub fn new_id(now_ms: i64) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("hist-{now_ms}-{seq}")
}

/// Default ceiling on remembered sessions.
pub const DEFAULT_HISTORY_CAP: usize = 200;

/// One finished cleanup session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    /// Epoch ms when the session finished.
    pub at_ms: i64,
    pub bytes: u64,
    pub items: usize,
    /// True for unattended (monitor / routine) cleanups.
    pub automatic: bool,
    /// Short labels of what was removed.
    pub titles: Vec<String>,
}

/// Bounded, newest-first list of sessions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupHistory {
    entries: Vec<HistoryEntry>,
    cap: usize,
}

impl CleanupHistory {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            cap,
        }
    }

    pub fn from_entries(mut entries: Vec<HistoryEntry>, cap: usize) -> Self {
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.at_ms));
        entries.truncate(cap);
        Self { entries, cap }
    }

    /// Record a session, keeping the list newest-first and bounded.
    pub fn record(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(self.cap);
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
