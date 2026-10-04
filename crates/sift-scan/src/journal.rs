//! Resume journal contract.
//!
//! A scan can be interrupted. The journal makes the next scan a *resume*: it
//! persists each directory when it is discovered and again when its subtree
//! closes, so already-walked subtrees load as closed aggregates and only the
//! unfinished frontier is walked. Totals stay exact; nothing is double counted.
//!
//! This module is only the contract. `sift-scan` cannot depend on the SQLite
//! implementation (dependency direction), so the coordinator talks to
//! [`ScanJournal`]; the durable implementation lives in `sift-store`. Calls are
//! fast and non-blocking — an implementation hands records to a background
//! writer rather than doing I/O inline.

/// A directory captured the moment it becomes a child of its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoveredDir {
    /// Stable node key, stringified.
    pub key: String,
    /// Parent key, stringified. Empty for the scan root.
    pub parent_key: String,
    /// Raw directory name.
    pub name: Vec<u8>,
    /// Absolute path, used to re-open the directory on resume.
    pub path: String,
    pub depth: u16,
    pub mtime_ms: i64,
    pub dev: u64,
    pub ino: u64,
    pub is_symlink: bool,
    pub deletable: bool,
}

/// Final aggregate of a directory whose whole subtree was walked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClosedDir {
    /// Stable node key, stringified.
    pub key: String,
    pub logical: u64,
    pub physical: u64,
    pub files: u32,
}

/// A directory restored from a previous, interrupted scan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoredDir {
    pub discovered: DiscoveredDir,
    pub closed: bool,
    pub logical: u64,
    pub physical: u64,
    pub files: u32,
}

/// State restored for one scan root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoredScan {
    /// Directories ordered so parents precede children.
    pub dirs: Vec<RestoredDir>,
}

impl RestoredScan {
    pub fn new(dirs: Vec<RestoredDir>) -> Self {
        Self { dirs }
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// Persistence seam for resumable scans.
///
/// Implementations must be safe to call from the coordinator and must not
/// block the scan hot path. The default [`NullJournal`] records nothing, which
/// restores the original start-from-scratch behavior.
pub trait ScanJournal: Send + Sync {
    /// Record the scan root and start a new resumable session.
    fn begin(&self, root: &DiscoveredDir);

    /// Persist directories discovered in one listing of `scan_root`.
    fn discovered(&self, scan_root: &str, dirs: Vec<DiscoveredDir>);

    /// Persist final aggregates of directories that just closed.
    fn closed(&self, scan_root: &str, updates: Vec<ClosedDir>);

    /// Drop incomplete (still-open) directory records for `root` before a new
    /// scan loads the journal. Open directories are re-walked from scratch
    /// regardless, so they have no reuse value; they only accumulate whenever a
    /// previous scan was killed mid-run. Completed (closed) records survive and
    /// stay available for reuse. Default is a no-op.
    fn prune_open(&self, _root: &str) {}

    /// Load an earlier, interrupted session for `root`. Empty when nothing is
    /// resumable.
    fn load(&self, root: &str) -> RestoredScan;

    /// Forget state for `root` after a clean completion.
    fn complete(&self, root: &str);
}


impl DiscoveredDir {
    /// Build a discovered-directory record.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: String,
        parent_key: String,
        name: Vec<u8>,
        path: String,
        depth: u16,
        mtime_ms: i64,
        dev: u64,
        ino: u64,
        is_symlink: bool,
        deletable: bool,
    ) -> Self {
        Self {
            key,
            parent_key,
            name,
            path,
            depth,
            mtime_ms,
            dev,
            ino,
            is_symlink,
            deletable,
        }
    }
}

impl ClosedDir {
    pub const fn new(key: String, logical: u64, physical: u64, files: u32) -> Self {
        Self {
            key,
            logical,
            physical,
            files,
        }
    }
}


impl RestoredDir {
    pub fn new(
        discovered: DiscoveredDir,
        closed: bool,
        logical: u64,
        physical: u64,
        files: u32,
    ) -> Self {
        Self { discovered, closed, logical, physical, files }
    }
}

/// The no-op journal: original behavior, zero cost.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullJournal;

impl ScanJournal for NullJournal {
    fn begin(&self, _root: &DiscoveredDir) {}
    fn discovered(&self, _scan_root: &str, _dirs: Vec<DiscoveredDir>) {}
    fn closed(&self, _scan_root: &str, _updates: Vec<ClosedDir>) {}
    fn load(&self, _root: &str) -> RestoredScan {
        RestoredScan::default()
    }
    fn complete(&self, _root: &str) {}
}
