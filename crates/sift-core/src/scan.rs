//! Volumes, scan policy and the progress/event contract shared by every front
//! end.
//!
//! Nothing here knows about Tauri or any particular scan implementation:
//! the engine publishes these types, the adapters translate them into their own
//! event vocabulary, and both front ends describe the disk in the same words.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::id::VolumeId;
use crate::size::{format_bytes, ByteSize};

// ---- volumes ---------------------------------------------------------------

/// A mounted volume the user can scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Volume {
    pub id: VolumeId,
    /// Label shown in the location picker (`Macintosh HD`, `Backup`, …).
    pub name: String,
    pub mount_point: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    /// Reported filesystem name (`apfs`, `NTFS`, `ext4`, …); empty when unknown.
    pub file_system: String,
}

impl Volume {
    /// Construct a volume. Prefer the platform enumeration in `sift-platform`;
    /// this exists so adapters and tests can build volumes without fighting
    /// `#[non_exhaustive]`.
    pub fn new(
        id: VolumeId,
        name: impl Into<String>,
        mount_point: impl Into<PathBuf>,
        total_bytes: u64,
        available_bytes: u64,
        is_removable: bool,
        file_system: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            mount_point: mount_point.into(),
            total_bytes,
            available_bytes,
            is_removable,
            file_system: file_system.into(),
        }
    }

    /// Fraction already consumed, clamped to `0.0..=1.0`.
    pub fn used_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        (used as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }

    /// Bytes occupied, derived from total minus available.
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }

    /// Whether this is the volume the operating system booted from.
    pub fn is_system(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            let mount = self.mount_point.to_string_lossy();
            mount.eq_ignore_ascii_case("C:\\") || mount.eq_ignore_ascii_case("C:")
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.mount_point == Path::new("/")
        }
    }
}

impl fmt::Display for Volume {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({} free of {})",
            self.name,
            format_bytes(self.available_bytes),
            format_bytes(self.total_bytes)
        )
    }
}

// ---- scan policy -----------------------------------------------------------

/// How hard a scan should push.
///
/// The tiers exist because "fast" and "cheap" trade against each other and the
/// product has to be honest about which one it is spending. A rescan triggered
/// by a keystroke wants [`ScanPolicy::interactive`]; the first full pass over a
/// 2 TB volume wants [`ScanPolicy::thorough`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ScanPolicy {
    /// Worker threads. `0` means "decide from the machine".
    pub threads: u16,
    /// Refuse to follow symlinks that escape the scan root.
    pub stay_on_volume: bool,
    /// Read allocated blocks so physical size is exact (`st_blocks`).
    /// Costs one extra field read per file on platforms that report it inline.
    pub want_physical_size: bool,
    /// Retain a record for files at least this large so they can be listed and
    /// selected without re-reading the directory.
    pub file_detail_min_bytes: u64,
    /// Hardlink deduplication. Costs a hash entry per multiply-linked file.
    pub dedupe_hardlinks: bool,
    /// Optional depth ceiling for an overview scan. `0` means "no limit"
    /// (a full, exact walk). A positive value truncates discovery at that
    /// depth for a fast first pass; an exact pass can deepen afterwards.
    pub max_depth: u16,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        Self::thorough()
    }
}

impl ScanPolicy {
    /// Full pass: every core, exact physical size, hardlink-accurate.
    pub const fn thorough() -> Self {
        Self {
            threads: 0,
            stay_on_volume: true,
            want_physical_size: true,
            file_detail_min_bytes: u64::MAX,
            dedupe_hardlinks: true,
            max_depth: 0,
        }
    }

    /// Re-entering a directory the user just left: fewer threads, no file detail.
    pub const fn interactive() -> Self {
        Self {
            threads: 4,
            stay_on_volume: true,
            want_physical_size: false,
            file_detail_min_bytes: u64::MAX,
            dedupe_hardlinks: false,
            max_depth: 0,
        }
    }

    /// Keep a record for files at or above `bytes`.
    pub const fn with_file_detail(mut self, bytes: u64) -> Self {
        self.file_detail_min_bytes = bytes;
        self
    }

    /// Pin the worker count.
    pub const fn with_threads(mut self, threads: u16) -> Self {
        self.threads = threads;
        self
    }

    /// Resolved worker count for this policy on this machine.
    pub fn resolved_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads as usize;
        }
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        // Directory walking is syscall-bound with a high per-call latency; more
        // workers than cores keeps the I/O queue full without thrashing, but
        // past 4x the benefit disappears and lock contention grows.
        cores.clamp(2, 16)
    }
}

/// Why a scan stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanOutcome {
    /// Every reachable directory was walked.
    Completed,
    /// The caller cancelled.
    Cancelled,
    /// The scan hit a resource limit (node budget, time budget) and stopped
    /// early with partial results.
    BudgetExhausted,
    /// The root could not be opened.
    Failed,
}

impl ScanOutcome {
    pub const fn is_complete(self) -> bool {
        matches!(self, ScanOutcome::Completed)
    }
}

// ---- progress --------------------------------------------------------------

/// Live counters for a running scan.
///
/// Kept as plain integers updated by atomics so publishing progress never
/// allocates and never blocks a worker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Progress {
    /// Files visited.
    pub files: u64,
    /// Directories closed.
    pub dirs: u64,
    /// Directories that could not be read.
    pub denied: u64,
    /// Entries skipped because they were already counted hardlinks.
    pub hardlinks_skipped: u64,
    /// Directories skipped because they cross a volume boundary or were
    /// already visited through another path (e.g. a firmlink).
    pub boundary_skipped: u64,
    /// Bytes counted so far.
    pub bytes: ByteSize,
    /// Directories currently queued or in flight.
    pub queue_depth: u64,
    /// Fraction of the volume's in-use bytes accounted for, `0.0..=1.0`.
    pub coverage: f64,
    /// Seconds since the scan started.
    pub elapsed_secs: f64,
    /// Directories parked while the user decides an authorization prompt.
    pub awaiting: u64,
}

impl Progress {
    /// Visited entries per second, for the "should I wait?" question.
    pub fn entries_per_sec(&self) -> f64 {
        if self.elapsed_secs <= 0.0 {
            return 0.0;
        }
        (self.files + self.dirs) as f64 / self.elapsed_secs
    }

    /// Bytes per second.
    pub fn bytes_per_sec(&self) -> f64 {
        if self.elapsed_secs <= 0.0 {
            return 0.0;
        }
        self.bytes.dominant() as f64 / self.elapsed_secs
    }
}

// ---- scan requests and events ---------------------------------------------

/// Identifies one scan run. Events from an older run are rejected by comparing
/// this value, so a cancelled scan's trailing events cannot corrupt a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScanId(pub u64);

impl fmt::Display for ScanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scan-{}", self.0)
    }
}

/// What a front end asks the engine to do.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ScanRequest {
    pub id: ScanId,
    pub root: PathBuf,
    /// The directory the user is looking at; it is walked first.
    pub focus: PathBuf,
    pub policy: ScanPolicy,
    /// Bytes the volume reports as used, for the coverage fraction. `None`
    /// means "unknown" and the UI falls back to raw counters.
    pub volume_used_bytes: Option<u64>,
}

impl ScanRequest {
    pub fn new(id: ScanId, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            focus: root.clone(),
            id,
            root,
            policy: ScanPolicy::default(),
            volume_used_bytes: None,
        }
    }

    pub fn with_volume_used_bytes(mut self, used: u64) -> Self {
        self.volume_used_bytes = Some(used);
        self
    }

    pub fn with_focus(mut self, focus: impl Into<PathBuf>) -> Self {
        self.focus = focus.into();
        self
    }

    pub fn with_policy(mut self, policy: ScanPolicy) -> Self {
        self.policy = policy;
        self
    }
}

/// What the engine tells a front end.
///
/// Deliberately coarse: a directory is published once, when its size is final.
/// A per-file event stream was the original design and it spent more time
/// formatting messages than walking the disk.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ScanEvent {
    /// Restored directories from an interrupted scan, published before normal
    /// discovery so already-walked subtrees render immediately.
    NodesAdopted { scan: ScanId, nodes: Vec<AdoptedNode> },
    /// A directory's contents are known and its own total is final. It may
    /// still gain bytes from unvisited subdirectories, so `pending` stays true
    /// for the UI until `DirectoryClosed` or the scan ends.
    DirectoryListed { scan: ScanId, dir: DirectorySummary },
    /// A directory's subtree is fully accounted for.
    DirectoryClosed {
        scan: ScanId,
        key: crate::id::NodeKey,
        /// How the close should be presented.
        status: DirCloseStatus,
    },
    /// An interim total for a directory that is still filling in.
    DirectorySized {
        scan: ScanId,
        key: crate::id::NodeKey,
        size: ByteSize,
        files: u32,
        pending: bool,
        /// Totals are still an estimate pending exact calibration.
        estimated: bool,
    },
    /// Periodic counters. Emitted at most a few times a second.
    Progress { scan: ScanId, progress: Progress },
    /// The scan stopped.
    Finished {
        scan: ScanId,
        outcome: ScanOutcome,
        progress: Progress,
    },
    /// A non-fatal problem worth surfacing once (permission denied root, a
    /// volume that vanished, a full hardlink table).
    Warning { scan: ScanId, message: String },
    /// Exact totals for a previously predicted giant directory, measured after
    /// the main scan. The delta has already been propagated through the tree.
    Calibrated {
        scan: ScanId,
        key: crate::id::NodeKey,
        size: ByteSize,
        files: u64,
    },
    /// Background calibration has finished.
    CalibrationFinished { scan: ScanId },
    /// A directory needs an OS authorization decision (macOS TCC). The scan
    /// parks it instead of skipping: the front end prompts the user, then the
    /// decision is fed back to the engine, which either re-walks the
    /// directory or closes it as explicitly skipped.
    PermissionRequested {
        scan: ScanId,
        key: crate::id::NodeKey,
        /// Absolute path the user should select in the open panel.
        path: String,
        /// Display name of the directory.
        name: String,
        /// `true` when the denial comes from macOS TCC (an open-panel user
        /// selection grants it); `false` for POSIX owner/mode denials that
        /// need an administrator.
        tcc: bool,
    },
    /// A post-completion authorization grant finished re-walking the subtree;
    /// totals were already patched through the tree and its ancestors. The
    /// front end uses this to refresh analysis without a new full scan.
    RefreshFinished {
        scan: ScanId,
        /// The directory that was re-walked.
        path: String,
    },
}

/// How a directory's final close is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirCloseStatus {
    /// Walked completely; its size is exact.
    Ok,
    /// Unreadable or explicitly skipped by the user; size is unknown.
    Denied,
    /// Settled at zero pending an authorization decision the user can make
    /// after the scan completes.
    Awaiting,
}

/// One directory restored from a previous interrupted scan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AdoptedNode {
    pub key: String,
    pub parent_key: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub deletable: bool,
    /// `false` while its subtree was unfinished and will be re-walked.
    pub closed: bool,
}

impl AdoptedNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: String,
        parent_key: String,
        name: String,
        path: String,
        size: u64,
        mtime_ms: i64,
        deletable: bool,
        closed: bool,
    ) -> Self {
        Self { key, parent_key, name, path, size, mtime_ms, deletable, closed }
    }
}

impl ScanEvent {
    pub fn scan_id(&self) -> ScanId {
        match self {
            ScanEvent::NodesAdopted { scan, .. } => *scan,
            ScanEvent::DirectoryListed { scan, .. }
            | ScanEvent::DirectoryClosed { scan, .. }
            | ScanEvent::DirectorySized { scan, .. }
            | ScanEvent::Progress { scan, .. }
            | ScanEvent::Finished { scan, .. }
            | ScanEvent::Warning { scan, .. }
            | ScanEvent::Calibrated { scan, .. }
            | ScanEvent::CalibrationFinished { scan, .. }
            | ScanEvent::PermissionRequested { scan, .. }
            | ScanEvent::RefreshFinished { scan, .. } => *scan,
        }
    }
}

/// One row of a listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectoryEntry {
    pub key: crate::id::NodeKey,
    pub name: String,
    pub is_dir: bool,
    pub size: ByteSize,
    pub mtime_ms: i64,
    pub pending: bool,
    pub deletable: bool,
}

impl DirectoryEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: crate::id::NodeKey,
        name: impl Into<String>,
        is_dir: bool,
        size: ByteSize,
        mtime_ms: i64,
        pending: bool,
        deletable: bool,
    ) -> Self {
        Self {
            key,
            name: name.into(),
            is_dir,
            size,
            mtime_ms,
            pending,
            deletable,
        }
    }
}

/// A directory and its direct children, published as one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectorySummary {
    pub key: crate::id::NodeKey,
    pub path: String,
    pub size: ByteSize,
    pub file_count: u32,
    pub dir_count: u32,
    pub entries: Vec<DirectoryEntry>,
}

impl DirectorySummary {
    /// Build a summary. Exists so adapters and the engine can construct the
    /// `#[non_exhaustive]` type without a struct literal.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: crate::id::NodeKey,
        path: impl Into<String>,
        size: ByteSize,
        file_count: u32,
        dir_count: u32,
        entries: Vec<DirectoryEntry>,
    ) -> Self {
        Self {
            key,
            path: path.into(),
            size,
            file_count,
            dir_count,
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(total: u64, available: u64) -> Volume {
        Volume {
            id: VolumeId::from_mount_point(Path::new("/")),
            name: "Macintosh HD".into(),
            mount_point: PathBuf::from("/"),
            total_bytes: total,
            available_bytes: available,
            is_removable: false,
            file_system: "apfs".into(),
        }
    }

    #[test]
    fn used_ratio_clamps_at_both_ends() {
        assert_eq!(volume(100, 40).used_ratio(), 0.6);
        assert_eq!(volume(100, 0).used_ratio(), 1.0);
        // A filesystem reporting more free than total must not produce NaN.
        assert_eq!(volume(100, 200).used_ratio(), 0.0);
        assert_eq!(volume(0, 0).used_ratio(), 0.0);
    }

    #[test]
    fn system_volume_detection() {
        assert!(volume(100, 50).is_system());
        let mut other = volume(100, 50);
        other.mount_point = PathBuf::from("/Volumes/Backup");
        assert!(!other.is_system());
    }

    #[test]
    fn scan_policy_resolves_threads() {
        assert_eq!(ScanPolicy::interactive().resolved_threads(), 4);
        let threads = ScanPolicy::thorough().resolved_threads();
        assert!((2..=16).contains(&threads), "got {threads}");
    }

    #[test]
    fn policy_builders_compose() {
        let policy = ScanPolicy::thorough()
            .with_threads(6)
            .with_file_detail(1024);
        assert_eq!(policy.threads, 6);
        assert_eq!(policy.file_detail_min_bytes, 1024);
        assert!(policy.want_physical_size);
    }

    #[test]
    fn scan_request_defaults_focus_to_root() {
        let request = ScanRequest::new(ScanId(1), "/Users/me");
        assert_eq!(request.focus, request.root);
        let focused = request.with_focus("/Users/me/Downloads");
        assert_eq!(focused.focus, PathBuf::from("/Users/me/Downloads"));
    }

    #[test]
    fn progress_rates_are_safe_at_zero_elapsed() {
        let progress = Progress {
            files: 100,
            elapsed_secs: 0.0,
            ..Progress::default()
        };
        assert_eq!(progress.entries_per_sec(), 0.0);
        assert_eq!(progress.bytes_per_sec(), 0.0);
    }

    #[test]
    fn progress_rates_are_computed() {
        let progress = Progress {
            files: 1000,
            dirs: 100,
            bytes: ByteSize::logical_only(2048),
            elapsed_secs: 2.0,
            ..Progress::default()
        };
        assert_eq!(progress.entries_per_sec(), 550.0);
        assert_eq!(progress.bytes_per_sec(), 1024.0);
    }

    #[test]
    fn event_carries_its_scan_id() {
        let event = ScanEvent::Progress {
            scan: ScanId(7),
            progress: Progress::default(),
        };
        assert_eq!(event.scan_id(), ScanId(7));
        let finished = ScanEvent::Finished {
            scan: ScanId(9),
            outcome: ScanOutcome::Cancelled,
            progress: Progress::default(),
        };
        assert_eq!(finished.scan_id(), ScanId(9));
        assert!(!ScanOutcome::Cancelled.is_complete());
        assert!(ScanOutcome::Completed.is_complete());
    }
}
