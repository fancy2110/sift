//! Snapshot contract for scan resumption and incremental rescans.
//!
//! The engine treats a snapshot store as an optional optimisation:
//!
//! * Before reading a directory, a worker stats it once and asks the store for
//!   a snapshot whose `mtime_ms` matches. A hit means the directory's *whole
//!   subtree* is reused without enumeration — the directory mtime is the
//!   filesystem's own structural-change signal, so this is exact, not a guess.
//! * When a directory closes, the coordinator hands its final totals to the
//!   store. `record` must return promptly; implementations are expected to
//!   buffer and write off the coordinator's thread.
//!
//! A crash needs no special bookkeeping. Only fully-closed directories are
//! ever recorded, so an interrupted scan resumes by simply walking again:
//! finished subtrees are matched and skipped, unfinished ones are re-read.
//!
//! A user-visible "full rescan" command bypasses the store entirely.

use std::path::{Path, PathBuf};

/// A closed directory's durable summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirSnapshot {
    /// Absolute path, primary key.
    pub path: PathBuf,
    /// The directory's own modification time in unix milliseconds.
    pub mtime_ms: i64,
    /// Logical bytes of the whole subtree. Signed because SQLite stores
    /// integers as i64; byte counts never approach the i64 maximum.
    pub logical: i64,
    /// Physical (allocated) bytes of the whole subtree.
    pub physical: i64,
    /// Files anywhere in the subtree.
    pub files: i64,
}

impl DirSnapshot {
    pub fn new(
        path: impl Into<PathBuf>,
        mtime_ms: i64,
        logical: i64,
        physical: i64,
        files: i64,
    ) -> Self {
        Self {
            path: path.into(),
            mtime_ms,
            logical,
            physical,
            files,
        }
    }
}

/// Persists directory snapshots. Implementations must be cheap to call from
/// hot engine threads: `lookup` runs on workers, `record` on the coordinator.
pub trait SnapshotStore: Send + Sync {
    /// Return the stored subtree summary for `path` when it was recorded with
    /// exactly `mtime_ms`. A mismatch or miss returns `None`.
    fn lookup(&self, path: &Path, mtime_ms: i64) -> Option<DirSnapshot>;

    /// Remember one closed directory. Must not block on disk I/O; an
    /// implementation may drop records under pressure (a missed snapshot only
    /// costs a re-read, never correctness).
    fn record(&self, snapshot: DirSnapshot);
}

/// Shared stores sit behind an `Arc` in the engine.
impl<T: SnapshotStore + ?Sized> SnapshotStore for std::sync::Arc<T> {
    fn lookup(&self, path: &Path, mtime_ms: i64) -> Option<DirSnapshot> {
        (**self).lookup(path, mtime_ms)
    }

    fn record(&self, snapshot: DirSnapshot) {
        (**self).record(snapshot)
    }
}

/// A directory entry's mtime as a cheap standalone stat, the one extra syscall
/// a snapshot probe costs. `None` when it cannot be obtained.
pub fn stat_mtime_ms(path: &Path) -> Option<i64> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(metadata_mtime_ms(&metadata))
}

/// Extract unix-millisecond mtime from already-fetched metadata.
pub fn metadata_mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .map(modified_millis)
        .unwrap_or(0)
}

fn modified_millis(modified: std::time::SystemTime) -> i64 {
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
