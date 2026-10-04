//! The scan engine.
//!
//! ## Shape
//!
//! A coordinator thread owns the [`ScanTree`] and every byte counter; a pool of
//! dumb worker threads owns the I/O. Workers pull jobs from a priority queue
//! (owned by the coordinator, which is the only writer), read directories with
//! the platform's fastest bulk enumeration, and hand the raw entries back. The
//! coordinator never touches the filesystem and the workers never touch the
//! tree, so the tree needs no locks of its own — one `Mutex` around it exists
//! only for the UI to read live state.
//!
//! ```text
//!        priority queue (coordinator-owned)
//!           ▲      │ dispatch (bounded by worker count)
//!           │      ▼
//!      coordinator     workers ── read_dir / getattrlistbulk ──► raw entries
//!      (builds tree,   │  (N threads, I/O-bound)
//!       publishes)     └────────────────────────────────────────► coordinator
//! ```
//!
//! ## Priorities
//!
//! The focus directory is published within milliseconds of `scan()` returning
//! because the queue is a heap keyed by [`crate::priority::category`]: the
//! focus (0) beats its ancestors (1), which beat its siblings (2), which beat
//! its descendants (3), which beat everything else (4). The user's current
//! directory is usable long before the volume finishes.
//!
//! ## Budgets
//!
//! [`sift_core::TreeConfig`] caps node records and depth. A directory refused
//! by a budget is still walked — as a *size-only* job that sums bytes on the
//! worker without creating tree nodes — so totals stay exact while memory stays
//! bounded.
//!
//! ## Correctness notes
//!
//! * Hardlinked files are counted once per (device, inode) when enabled.
//! * Logical and physical bytes are tracked separately and never conflated.
//! * A directory's size is final the moment its last child closes, and it is
//!   rolled into its parent exactly once ([`sift_core::ScanTree::close_dir`] is
//!   idempotent).

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use sift_core::id::{FileId, NodeKey};
use sift_core::tree::NONE;
use sift_core::{
    ByteSize, DirCloseStatus, DirectoryEntry, DirectorySummary, Progress, ScanEvent, ScanId,
    ScanOutcome, ScanPolicy, ScanRequest, ScanTree, TreeConfig,
};
use sift_platform::dir::{
    close_raw_fd, fd_size, path_size, read_with_children, BulkBuffer, ChildFdMap, DirReader,
    RawEntry, ReadErrorKind,
};

use crate::priority::{category, JobKind, OrdDir, QueuedDir};

static WORKER_BUSY_NS: AtomicU64 = AtomicU64::new(0);
static COORD_PROCESS_NS: AtomicU64 = AtomicU64::new(0);

/// Diagnostics: aggregate worker busy time and coordinator processing time.
pub fn timing_stats() -> (u64, u64) {
    (
        WORKER_BUSY_NS.load(AtomicOrdering::Relaxed),
        COORD_PROCESS_NS.load(AtomicOrdering::Relaxed),
    )
}

/// Reset the timing accumulators (bench harness, before each run).
pub fn reset_timing_stats() {
    WORKER_BUSY_NS.store(0, AtomicOrdering::Relaxed);
    COORD_PROCESS_NS.store(0, AtomicOrdering::Relaxed);
}
use crate::journal::{
    ClosedDir, DiscoveredDir, NullJournal, ScanJournal,
};
use sift_core::scan::AdoptedNode;

/// How often the coordinator publishes interim progress and size updates.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);


/// The engine. Cheap to create and share.
#[derive(Debug, Clone)]
pub struct ScanEngine {
    workers: usize,
}

/// A running scan: the event stream, the live tree, and the control handles.
pub struct ScanHandle {
    pub scan_id: ScanId,
    /// Events from the coordinator, drained by an adapter thread or task.
    pub events: mpsc::Receiver<ScanEvent>,
    /// The tree as it grows. Lock briefly to read; the coordinator mutates
    /// under the same lock.
    pub tree: Arc<Mutex<ScanTree>>,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
    permission_tx: crossbeam_channel::Sender<PermissionDecision>,
    stop: Arc<AtomicBool>,
    /// Set when the coordinator thread exits.
    pub finished: Arc<AtomicBool>,
}

/// A cloneable control handle for a running scan.
///
/// Separated from [`ScanHandle`] because the event receiver is single-consumer:
/// a supervisor (a UI state object, an IPC layer) needs to cancel and re-focus a
/// scan without owning the stream, and can hold this instead.
#[derive(Clone)]
pub struct ScanControl {
    scan_id: ScanId,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
    permission_tx: crossbeam_channel::Sender<PermissionDecision>,
    /// Set to make the lingering coordinator end its post-completion wait.
    stop: Arc<AtomicBool>,
}

/// A user decision fed back to an authorization-pending directory.
#[derive(Debug, Clone)]
pub struct PermissionDecision {
    /// Node key (the front end's node id) of the parked directory.
    pub key: String,
    /// `true` — access granted, re-walk the directory;
    /// `false` — the user explicitly skipped it.
    pub granted: bool,
    /// When granted, the folder the user actually selected in the panel. May be
    /// an ancestor of the parked directory (e.g. `~/Library`), in which case
    /// its TCC grant covers every other parked directory below it.
    pub granted_path: Option<String>,
}

impl ScanControl {
    /// The scan run this control belongs to; lets an adapter ignore stale
    /// handles from a scan that has since been replaced.
    pub fn scan_id(&self) -> ScanId {
        self.scan_id
    }

    /// End the coordinator's post-completion authorization wait, if it is in
    /// one. Used when a new scan replaces this one and the old handle lingers.
    pub fn shutdown(&self) {
        self.stop.store(true, AtomicOrdering::SeqCst);
    }

    /// Ask the scan to stop soon. Workers finish their current directory read,
    /// then drain; a `Finished { outcome: Cancelled }` event follows.
    pub fn cancel(&self) {
        self.cancel.store(true, AtomicOrdering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::SeqCst)
    }

    /// Pause discovery. Jobs already dispatched finish (so sizes stay exact),
    /// but no new directories are dispatched until [`ScanControl::resume`].
    pub fn pause(&self) {
        self.paused.store(true, AtomicOrdering::SeqCst);
    }

    /// Resume a paused scan.
    pub fn resume(&self) {
        self.paused.store(false, AtomicOrdering::SeqCst);
    }

    /// Whether discovery is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(AtomicOrdering::SeqCst)
    }

    /// Move the scan's priority to `focus` for directories not yet queued.
    pub fn set_focus(&self, focus: impl Into<PathBuf>) {
        *self.focus.lock().unwrap() = focus.into();
        self.focus_generation.fetch_add(1, AtomicOrdering::SeqCst);
    }

    /// The focus the scan is currently prioritizing.
    pub fn focus(&self) -> PathBuf {
        self.focus.lock().unwrap().clone()
    }

    /// Feed back the user's decision on an authorization prompt. Fails only
    /// when the scan has already ended.
    pub fn resolve_permission(
        &self,
        key: impl Into<String>,
        granted: bool,
        granted_path: Option<String>,
    ) -> Result<(), crossbeam_channel::SendError<PermissionDecision>> {
        self.permission_tx.send(PermissionDecision {
            key: key.into(),
            granted,
            granted_path,
        })
    }
}

impl std::fmt::Debug for ScanControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanControl")
            .field("focus", &self.focus())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl ScanHandle {
    /// A cloneable control handle, for a supervisor that does not own the events.
    pub fn control(&self) -> ScanControl {
        ScanControl {
            scan_id: self.scan_id,
            cancel: Arc::clone(&self.cancel),
            paused: Arc::clone(&self.paused),
            focus: Arc::clone(&self.focus),
            focus_generation: Arc::clone(&self.focus_generation),
            permission_tx: self.permission_tx.clone(),
            stop: Arc::clone(&self.stop),
        }
    }

    /// Ask the scan to stop soon. Workers finish their current directory read,
    /// then drain; a `Finished { outcome: Cancelled }` event follows.
    pub fn cancel(&self) {
        self.control().cancel();
    }

    /// Move the scan's priority to `focus` for directories not yet queued.
    pub fn set_focus(&self, focus: impl Into<PathBuf>) {
        self.control().set_focus(focus);
    }

    /// Block until the coordinator thread finishes. Returns the outcome.
    pub fn join(&self) -> ScanOutcome {
        while !self.finished.load(AtomicOrdering::SeqCst) {
            std::thread::yield_now();
        }
        // The last event carries the outcome.
        let mut last = ScanOutcome::Failed;
        while let Ok(event) = self.events.try_recv() {
            if let ScanEvent::Finished { outcome, .. } = event {
                last = outcome;
            }
        }
        last
    }
}

impl Default for ScanEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanEngine {
    pub fn new() -> Self {
        Self {
            workers: sift_platform::recommended_scan_workers(),
        }
    }

    /// An engine pinned to `workers` threads, for tests and constrained boxes.
    pub fn with_workers(workers: usize) -> Self {
        Self {
            workers: workers.clamp(1, 64),
        }
    }

    /// Start scanning `request`. Returns immediately; events arrive on the
    /// handle's receiver from a background coordinator.
    pub fn scan(&self, request: ScanRequest) -> std::io::Result<ScanHandle> {
        self.scan_with_journal(request, Arc::new(NullJournal))
    }

    /// Start scanning with an explicit resume journal. With [`NullJournal`]
    /// this is a plain scan; with a durable journal an interrupted scan is
    /// adopted and resumed.
    pub fn scan_with_journal(
        &self,
        request: ScanRequest,
        journal: Arc<dyn ScanJournal>,
    ) -> std::io::Result<ScanHandle> {
        // An explicitly pinned policy wins; otherwise the engine's count (the
        // recommended performance-core count for a default engine) applies.
        let workers = if request.policy.threads > 0 {
            request.policy.threads as usize
        } else {
            self.workers.max(1)
        };
        let config = tree_config_for(&request.policy);

        let root_key = NodeKey::from_path(&request.root);
        let (root_dev, root_ino) = root_identity(&request.root);

        let root_text = request.root.to_string_lossy().to_string();
        // Discard the incomplete frontier from a previously killed scan before
        // loading; only fully-closed subtrees are worth restoring.
        journal.prune_open(&root_text);
        let restored = journal.load(&root_text);
        let adopting = !restored.is_empty();
        let mut tree_inner = ScanTree::new(root_key, &request.root, config);

        // Maps a restored node key to its live index, plus the sets that tell
        // the re-list which children are already in the tree.
        let mut adopted_index: HashMap<String, u32> = HashMap::new();
        let mut adopted_closed: HashSet<String> = HashSet::new();
        let mut adopted_nodes: Vec<AdoptedNode> = Vec::new();
        let mut seed_pending: HashMap<u32, u32> = HashMap::new();

        if adopting {
            // The scan root index already exists (0) and stays open.
            adopted_index.insert(root_key.to_string(), 0);
            for restored_dir in &restored.dirs {
                let d = &restored_dir.discovered;
                let name_string = String::from_utf8_lossy(&d.name).into_owned();
                let parent_index = adopted_index
                    .get(&d.parent_key)
                    .copied()
                    .unwrap_or(0);
                let aggregate = ByteSize::new(restored_dir.logical, restored_dir.physical);
                let node_index = tree_inner.adopt_node(
                    parent_index,
                    NodeKey::from_hex(&d.key).unwrap_or(root_key),
                    &d.name,
                    d.mtime_ms,
                    d.deletable,
                    d.is_symlink,
                    restored_dir.closed,
                    aggregate,
                    restored_dir.files,
                );
                if let Some(node_index) = node_index {
                    adopted_index.insert(d.key.clone(), node_index);
                    if restored_dir.closed {
                        adopted_closed.insert(d.key.clone());
                    } else {
                        *seed_pending.entry(parent_index).or_insert(0) += 1;
                    }
                    adopted_nodes.push(AdoptedNode::new(
                        d.key.clone(),
                        d.parent_key.clone(),
                        name_string,
                        d.path.clone(),
                        if restored_dir.physical > 0 {
                            restored_dir.physical
                        } else {
                            restored_dir.logical
                        },
                        d.mtime_ms,
                        d.deletable,
                        restored_dir.closed,
                    ));
                }
            }
        }

        // Seed the live counters from restored subtrees so a resumed scan's
        // progress starts where the previous run left off, instead of at zero.
        // Bytes are summed only over closed subtrees whose parent is not itself
        // closed: a closed parent's size already includes every closed child.
        // Those subtrees are never re-walked, so file accounting will not
        // count them again.
        let mut seed_bytes = ByteSize::ZERO;
        let mut seed_files: u64 = 0;
        let mut seed_dirs: u64 = 0;
        if adopting {
            for restored_dir in &restored.dirs {
                if !restored_dir.closed {
                    continue;
                }
                seed_dirs += 1;
                if !adopted_closed.contains(&restored_dir.discovered.parent_key) {
                    seed_bytes.logical = seed_bytes
                        .logical
                        .saturating_add(restored_dir.logical);
                    seed_bytes.physical = seed_bytes
                        .physical
                        .saturating_add(restored_dir.physical);
                    seed_files += restored_dir.files as u64;
                }
            }
        }

        let tree = Arc::new(Mutex::new(tree_inner));
        let cancel = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let focus = Arc::new(Mutex::new(request.focus.clone()));
        let focus_generation = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        // One shared multi-producer/multi-consumer queue. Every idle worker
        // pulls the next job itself, giving dynamic load balancing without
        // serializing behind a mutex: a worker blocked in a long size-only
        // subtree walk does not get more jobs while the others stay fed.
        let (dispatch_tx, dispatch_rx) = crossbeam_channel::unbounded::<DirJob>();
        let (result_tx, result_rx) = mpsc::channel::<DirResult>();
        let (event_tx, event_rx) = mpsc::channel::<ScanEvent>();
        let (permission_tx, permission_rx) =
            crossbeam_channel::unbounded::<PermissionDecision>();

        // Workers: pull jobs, read directories, return raw entries. The MPMC
        // receiver is shared by clone; each worker blocks on it independently.
        let want_physical = request.policy.want_physical_size;
        for _ in 0..workers {
            let dispatch_rx = dispatch_rx.clone();
            let result_tx = result_tx.clone();
            let cancel = Arc::clone(&cancel);
            std::thread::Builder::new()
                .name("sift-worker".into())
                .spawn(move || {
                    promote_thread_qos();
                    let mut buffer = BulkBuffer::new();
                    loop {
                    let Ok(job) = dispatch_rx.recv() else { break };
                    if cancel.load(AtomicOrdering::SeqCst) {
                        break;
                    }
                    let busy_start = Instant::now();
                    let result = read_job(job, want_physical, &mut buffer);
                    let busy = busy_start.elapsed().as_nanos() as u64;
                    WORKER_BUSY_NS.fetch_add(busy, AtomicOrdering::Relaxed);
                    if result_tx.send(result).is_err() {
                        break;
                    }
                    }
                })?;
        }
        drop(result_tx);

        // Coordinator: owns the queue, the tree, and the event stream.
        let tree_co = Arc::clone(&tree);
        let cancel_co = Arc::clone(&cancel);
        let paused_co = Arc::clone(&paused);
        let focus_co = Arc::clone(&focus);
        let focus_generation_co = Arc::clone(&focus_generation);
        let finished_co = Arc::clone(&finished);
        let stop_co = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("sift-coordinator".into())
            .spawn(move || {
                promote_thread_qos();
                run_coordinator(CoordinatorInputs {
                    scan_id: request.id,
                    workers,
                    root_key,
                    root_text: root_text.clone(),
                    journal: Arc::clone(&journal),
                    adopted_nodes: std::mem::take(&mut adopted_nodes),
                    adopted_closed: std::mem::take(&mut adopted_closed),
                    adopted_index: std::mem::take(&mut adopted_index),
                    seed_pending: std::mem::take(&mut seed_pending),
                    seed_bytes,
                    seed_files,
                    seed_dirs,
                    root_dev,
                    root_ino,
                    root_path: request.root.clone(),
                    focus_path: request.focus.clone(),
                    volume_used: request.volume_used_bytes,
                    tree: tree_co,
                    cancel: cancel_co,
                    paused: paused_co,
                    focus: focus_co,
                    focus_generation: focus_generation_co,
                    dispatch_tx,
                    result_rx,
                    event_tx,
                    permission_rx,
                    stop: stop_co,
                    finished: finished_co,
                });
            })?;

        Ok(ScanHandle {
            scan_id: request.id,
            events: event_rx,
            tree,
            cancel,
            paused,
            focus,
            focus_generation,
            permission_tx,
            stop: Arc::clone(&stop),
            finished,
        })
    }
}

struct CoordinatorInputs {
    scan_id: ScanId,
    workers: usize,
    root_key: NodeKey,
    root_text: String,
    journal: Arc<dyn ScanJournal>,
    /// Restored nodes already published to the UI at adoption time.
    adopted_nodes: Vec<AdoptedNode>,
    adopted_closed: HashSet<String>,
    adopted_index: HashMap<String, u32>,
    /// Per-parent pending counts for adopted (already-in-tree) open dirs.
    seed_pending: HashMap<u32, u32>,
    /// Bytes/files/dirs already known from restored closed subtrees.
    seed_bytes: ByteSize,
    seed_files: u64,
    seed_dirs: u64,
    root_dev: u64,
    root_ino: u64,
    root_path: PathBuf,
    focus_path: PathBuf,
    volume_used: Option<u64>,
    tree: Arc<Mutex<ScanTree>>,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
    dispatch_tx: crossbeam_channel::Sender<DirJob>,
    result_rx: mpsc::Receiver<DirResult>,
    event_tx: mpsc::Sender<ScanEvent>,
    permission_rx: crossbeam_channel::Receiver<PermissionDecision>,
    stop: Arc<AtomicBool>,
    /// Set once the main walk finishes and `Finished` is published, even while
    /// the coordinator lingers for post-completion grants.
    finished: Arc<AtomicBool>,
}

struct CoordinatorState {
    queue: BinaryHeap<OrdDir>,
    /// Subdirectories of a tracked dir that are enqueued but not yet closed.
    pending_children: HashMap<u32, u32>,
    /// Size-only jobs in flight per parent; the parent must wait for them.
    size_only_pending: HashMap<u32, u32>,
    /// Accumulated bytes of unrecorded subtrees, folded when the parent closes.
    unrecorded: HashMap<u32, (ByteSize, u32)>,
    /// Tracked dirs whose size changed since the last progress flush.
    dirty: HashSet<u32>,
    /// Global counters published in `Progress`.
    progress: Progress,
    /// Tracked directory nodes currently open (created but not yet closed),
    /// including the root and parked/awaiting directories.
    open_dirs: u64,
    /// Highest coverage published so far; keeps the percent monotonic even
    /// when listing an open directory discovers a large new subtree.
    percent_peak: f64,
    seq: u64,
    in_flight: usize,
    started: Instant,
    last_flush: Instant,
    focus: PathBuf,
    focus_seen_generation: u64,
    /// The focus the scan started with; until this directory is listed, the
    /// coordinator dispatches one job at a time so the user's view is
    /// deterministic-first instead of racing the rest of the volume.
    initial_focus: PathBuf,
    focus_processed: bool,
    cancelled: bool,
    paused: bool,
    volume_used: Option<u64>,
    event_tx: mpsc::Sender<ScanEvent>,
    scan_id: ScanId,
    /// Device of the scan root; subdirectories on another device are skipped.
    root_dev: u64,
    /// (device, inode) of every directory already reached, so a directory
    /// encountered through a firmlink and again through its mount point is
    /// walked only once.
    visited_dirs: HashSet<(u64, u64)>,
    /// Durable resume journal.
    journal: Arc<dyn ScanJournal>,
    /// Absolute scan root text, used as the journal partition key.
    root_text: String,
    /// Restored node keys already installed. The re-list skips these when they
    /// are closed (no recount, no re-enqueue); open ones re-walk.
    adopted_closed: HashSet<String>,
    /// Restored nodes already published, so they are not re-published.
    adopted_set: HashSet<String>,
    /// Restored key → live index, so the re-list can re-enqueue adopted open
    /// dirs at their existing nodes.
    adopted_index: HashMap<String, u32>,
    /// Predicted giant directories awaiting exact post-scan calibration.
    calibrate: Vec<CalibrationJob>,
    /// Directories parked while the user decides an authorization prompt,
    /// keyed by their node-key string so decisions from a front end resolve
    /// without a key→index lookup.
    awaiting: HashMap<String, AwaitingDir>,
    /// Incoming user decisions for parked directories.
    permission_rx: crossbeam_channel::Receiver<PermissionDecision>,
    /// Granted subtrees currently being re-walked; their ancestor chain is
    /// already closed, so they close through the refresh path instead of the
    /// normal pending-slot bookkeeping.
    refresh_roots: HashSet<u32>,
    /// Every directory belonging to a refresh walk (the root + all reached or
    /// opened descendants). Listing one of these consults the existing-child
    /// cache so closed nodes are re-walked in place instead of duplicated.
    refresh_open: HashSet<u32>,
    /// Per-parent cache of existing children by raw name, built on demand. Lets
    /// a refresh walk find a node to reuse without O(n²) sibling scans on a
    /// giant flat directory.
    existing_cache: HashMap<u32, HashMap<Vec<u8>, u32>>,
    /// Set when a replacing scan asks this lingering coordinator to stop.
    stop: Arc<AtomicBool>,
}

/// One directory settled at zero while awaiting an authorization decision.
#[derive(Clone)]
struct AwaitingDir {
    index: u32,
    path: String,
    name: String,
}

fn run_coordinator(inputs: CoordinatorInputs) {
    let CoordinatorInputs {
        scan_id,
        workers,
        root_key,
        root_text,
        journal,
        adopted_nodes,
        adopted_closed,
        adopted_index,
        seed_pending,
        seed_bytes,
        seed_files,
        seed_dirs,
        root_dev,
        root_ino,
        root_path,
        focus_path,
        volume_used,
        tree,
        cancel,
        paused,
        focus,
        focus_generation,
        dispatch_tx,
        result_rx,
        event_tx,
        permission_rx,
        stop,
        finished,
    } = inputs;

    // Root (index 0) is open from the start; every adopted node whose subtree
    // was unfinished is also open and will be re-walked.
    let seed_open = 1 + adopted_nodes.iter().filter(|node| !node.closed).count() as u64;

    let mut state = CoordinatorState {
        queue: BinaryHeap::new(),
        pending_children: HashMap::new(),
        size_only_pending: HashMap::new(),
        unrecorded: HashMap::new(),
        dirty: HashSet::new(),
        progress: {
            let mut progress = Progress::default();
            progress.bytes = seed_bytes;
            progress.files = seed_files;
            progress.dirs = seed_dirs;
            progress
        },
        open_dirs: seed_open,
        percent_peak: 0.0,
        seq: 0,
        in_flight: 0,
        started: Instant::now(),
        last_flush: Instant::now(),
        focus: focus_path.clone(),
        focus_seen_generation: 0,
        initial_focus: focus_path.clone(),
        focus_processed: focus_path == root_path,
        cancelled: false,
        paused: false,
        volume_used,
        event_tx,
        scan_id,
        root_dev,
        visited_dirs: if root_dev != 0 && root_ino != 0 {
            HashSet::from([(root_dev, root_ino)])
        } else {
            HashSet::new()
        },
        journal,
        root_text: root_text.clone(),
        adopted_set: adopted_nodes
            .iter()
            .map(|node| node.key.clone())
            .collect(),
        adopted_closed,
        adopted_index,
        calibrate: Vec::new(),
        awaiting: HashMap::new(),
        permission_rx,
        refresh_roots: HashSet::new(),
        refresh_open: HashSet::new(),
        existing_cache: HashMap::new(),
        stop: Arc::clone(&stop),
    };

    // Resume: publish restored nodes before normal discovery, and tell each
    // parent which adopted (open) children it is still waiting for. Closed
    // adopted children are not pending — they already pre-charged the parent.
    if !adopted_nodes.is_empty() {
        let _ = state.event_tx.send(ScanEvent::NodesAdopted {
            scan: state.scan_id,
            nodes: adopted_nodes,
        });
    }
    for (parent, count) in seed_pending {
        *state.pending_children.entry(parent).or_insert(0) += count;
    }

    // Journal the scan root to (re)open the resumable session.
    let root_name_bytes = root_path
        .file_name()
        .map(|name| name.as_encoded_bytes().to_vec())
        .unwrap_or_else(|| root_path.as_os_str().as_encoded_bytes().to_vec());
    state.journal.begin(&DiscoveredDir::new(
        root_key.to_string(),
        String::new(),
        root_name_bytes,
        root_text.clone(),
        0,
        0,
        root_dev,
        root_ino,
        false,
        false,
    ));

    // Seed the queue with the root.
    let root_seq = state.next_seq();
    state.queue.push(OrdDir {
        queued: QueuedDir {
            index: 0,
            path: root_path.clone(),
            depth: 0,
            seq: root_seq,
            kind: JobKind::Tracked,
            writable: DirReader::probe_writable(&root_path),
        },
        category: category(&root_path, &focus_path),
        focus: focus_path,
        preopened: None,
    });

    let outcome = coordinator_loop(
        &mut state,
        &tree,
        &cancel,
        &paused,
        &focus,
        &focus_generation,
        &dispatch_tx,
        &result_rx,
        workers,
    );

    // Fold unrecorded totals so even a cancelled scan adds up.
    {
        let mut tree_guard = tree.lock().unwrap();
        for (parent, (size, files)) in state.unrecorded.iter() {
            tree_guard.add_unrecorded(*parent, *size, *files);
        }
    }

    // A clean completion clears the resume state; a cancellation leaves it for
    // the next scan to resume.
    if outcome == ScanOutcome::Completed {
        state.journal.complete(&state.root_text);
    }

    // Predicted giant directories are measured exactly after the main scan.
    // The background thread holds its own event sender, so `Finished` below
    // still precedes the `Calibrated` updates. Nothing is launched for a
    // cancellation: estimates stay estimates until the next scan.
    let calibrate = std::mem::take(&mut state.calibrate);
    if matches!(outcome, ScanOutcome::Completed) && !calibrate.is_empty() {
        let bg_tree = Arc::clone(&tree);
        let bg_event_tx = state.event_tx.clone();
        std::thread::Builder::new()
            .name("sift-calibration".into())
            .spawn(move || {
                run_background_calibration(
                    calibrate,
                    bg_tree,
                    bg_event_tx,
                    scan_id,
                    workers,
                );
            })
            .ok();
    }

    let final_progress = state.progress;
    let _ = (root_key,);

    let _ = state.event_tx.send(ScanEvent::Finished {
        scan: scan_id,
        outcome,
        progress: final_progress,
    });
    // The main walk is over even though this thread lingers; `join()` may
    // return and the UI may present results.
    finished.store(true, AtomicOrdering::SeqCst);

    // Linger for post-completion authorization grants. Unauthorized dirs were
    // settled at zero during the walk; a grant now starts a small refresh walk,
    // driven through the same worker pool (which is why `dispatch_tx` is kept),
    // and the patched totals arrive as DirectorySized + RefreshFinished.
    while !state.stop.load(AtomicOrdering::SeqCst) {
        match state.permission_rx.recv_timeout(PROGRESS_INTERVAL) {
            Ok(decision) => {
                apply_permission_decision(&mut state, &tree, decision);
                while let Ok(extra) = state.permission_rx.try_recv() {
                    apply_permission_decision(&mut state, &tree, extra);
                }
                // Drive any refresh walks the decisions started. Idle with
                // other awaiting dirs still present returns immediately.
                let _ = coordinator_loop(
                    &mut state,
                    &tree,
                    &cancel,
                    &paused,
                    &focus,
                    &focus_generation,
                    &dispatch_tx,
                    &result_rx,
                    workers,
                );
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Dropping every dispatch sender makes idle workers exit.
    drop(dispatch_tx);
    // A `Vec` that grew by doubling can hold nearly twice the nodes it needs.
    // The scan is over, so pay the one-time copy and give the memory back.
    tree.lock().unwrap().shrink_to_fit();
    drop(state.event_tx);
}

#[allow(clippy::too_many_arguments)]
fn coordinator_loop(
    state: &mut CoordinatorState,
    tree: &Arc<Mutex<ScanTree>>,
    cancel: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
    focus: &Arc<Mutex<PathBuf>>,
    focus_generation: &Arc<AtomicU64>,
    dispatch_tx: &crossbeam_channel::Sender<DirJob>,
    result_rx: &mpsc::Receiver<DirResult>,
    workers: usize,
) -> ScanOutcome {
    let mut outcome = ScanOutcome::Completed;
    loop {
        // Re-prioritise if the focus moved.
        let generation = focus_generation.load(AtomicOrdering::SeqCst);
        if generation != state.focus_seen_generation {
            state.focus_seen_generation = generation;
            state.focus = focus.lock().unwrap().clone();
            rebuild_categories(state);
        }

        if cancel.load(AtomicOrdering::SeqCst) {
            state.cancelled = true;
            outcome = ScanOutcome::Cancelled;
            break;
        }

        // A replacing scan asked this (possibly lingering) coordinator to end.
        if state.stop.load(AtomicOrdering::SeqCst) {
            state.cancelled = true;
            outcome = ScanOutcome::Cancelled;
            break;
        }

        // Apply any authorization decisions the front end sent back.
        while let Ok(decision) = state.permission_rx.try_recv() {
            apply_permission_decision(state, tree, decision);
        }

        state.paused = paused.load(AtomicOrdering::SeqCst);

        // Dispatch work while workers are free. Until the focus directory is
        // listed, keep one job in flight so focus-first is deterministic.
        // While paused, let in-flight jobs finish but dispatch nothing new.
        let dispatch_limit = if state.paused {
            0
        } else if state.focus_processed {
            workers
        } else {
            1
        };
        while state.in_flight < dispatch_limit {
            let Some(mut job) = state.queue.pop() else { break };
            let dispatch = match job.queued.kind {
                JobKind::Tracked => {
                    let preopened = job.preopened.take();
                    // The descriptor leaves the queue for a worker: it is now
                    // a transient slot, no longer counted against the queued
                    // cap.
                    if preopened.is_some() {
                        sift_platform::dir::adopt_queued_fd();
                    }
                    DirJob::Tracked {
                        index: job.queued.index,
                        path: std::mem::take(&mut job.queued.path),
                        writable: job.queued.writable,
                        preopened,
                    }
                }
                JobKind::SizeOnly => DirJob::SizeOnly {
                    parent: job.queued.index,
                    path: std::mem::take(&mut job.queued.path),
                },
            };
            if dispatch_tx.send(dispatch).is_err() {
                outcome = ScanOutcome::Failed;
                break;
            }
            state.in_flight += 1;
        }

        if state.in_flight == 0 {
            if state.paused {
                // Paused with every in-flight job settled. Do not treat the
                // idle state as completion: wait here, polling for resume or
                // cancel, so an indefinitely paused scan never finishes.
                while !cancel.load(AtomicOrdering::SeqCst)
                    && !state.stop.load(AtomicOrdering::SeqCst)
                    && paused.load(AtomicOrdering::SeqCst)
                {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                state.last_flush = Instant::now();
                continue;
            }
            // Queue empty and nothing in flight: every reachable directory was
            // walked. Directories awaiting an authorization decision already
            // settled at zero, so they never hold the tree open.
            break;
        }

        let result = match result_rx.recv() {
            Ok(result) => result,
            Err(_) => {
                outcome = if state.cancelled {
                    ScanOutcome::Cancelled
                } else {
                    ScanOutcome::Failed
                };
                break;
            }
        };
        state.in_flight -= 1;

        let process_start = Instant::now();
        process_result(state, tree, result);
        COORD_PROCESS_NS.fetch_add(
            process_start.elapsed().as_nanos() as u64,
            AtomicOrdering::Relaxed,
        );

        if state.last_flush.elapsed() >= PROGRESS_INTERVAL {
            flush_progress(state, tree);
            state.last_flush = Instant::now();
        }
    }

    flush_progress(state, tree);
    outcome
}

fn process_result(state: &mut CoordinatorState, tree: &Arc<Mutex<ScanTree>>, result: DirResult) {
    match result {
        DirResult::Tracked {
            index,
            writable,
            entries,
            child_fds,
        } => {
            match entries {
                Ok(entries) => {
                    let mut tree_guard = tree.lock().unwrap();
                    process_dir_entries(
                        state,
                        &mut tree_guard,
                        index,
                        writable,
                        &entries,
                        child_fds,
                    );
                    drop(tree_guard);
                }
                // Parked pending a user decision: do not close the directory
                // and do not touch pending_children, so its parent stays open.
                // TCC denials can be granted via an open panel; POSIX denials
                // are presented the same way but marked as needing an admin.
                Err(kind @ (ReadErrorKind::PermissionTcc
                | ReadErrorKind::PermissionPosix)) => {
                    park_for_permission(
                        state,
                        tree,
                        index,
                        child_fds,
                        matches!(kind, ReadErrorKind::PermissionTcc),
                    );
                }
                // NotFound, Other, or any future variant: settle at zero.
                Err(_) => {
                    let mut tree_guard = tree.lock().unwrap();
                    state.progress.denied += 1;
                    // Unreadable directory: mark it and settle it at zero.
                    if let Some(node) = tree_guard.node_mut(index) {
                        node.set_denied(true);
                    }
                    drop(child_fds);
                    close_and_cascade(state, &mut tree_guard, index);
                    drop(tree_guard);
                }
            }
        }
        DirResult::Sized {
            parent,
            size,
            files,
        } => {
            let mut tree_guard = tree.lock().unwrap();
            let finished_all = {
                let entry = state.size_only_pending.entry(parent).or_insert(0);
                *entry = entry.saturating_sub(1);
                *entry == 0
            };
            if finished_all {
                state.size_only_pending.remove(&parent);
            }
            state
                .unrecorded
                .entry(parent)
                .and_modify(|(s, f)| {
                    *s += size;
                    *f += files;
                })
                .or_insert((size, files));
            // If the parent is otherwise complete, close it now that its
            // unrecorded subtree bytes have arrived.
            if state.pending_children.get(&parent).copied().unwrap_or(0) == 0
                && !state.size_only_pending.contains_key(&parent)
            {
                close_and_cascade(state, &mut tree_guard, parent);
            }
            drop(tree_guard);
        }
        DirResult::Predicted { index } => {
            // The worker skipped the read; queue exact calibration and close
            // the node at zero so the main scan can complete.
            let path = {
                let mut tree_guard = tree.lock().unwrap();
                tree_guard.path(index)
            };
            state.calibrate.push(CalibrationJob {
                index,
                path: PathBuf::from(&path),
            });
            let mut tree_guard = tree.lock().unwrap();
            close_predicted(state, &mut tree_guard, index);
            drop(tree_guard);
        }
    }
}

/// Settle a directory whose read failed with a permission error at zero, keep
/// an authorization card for it, and continue the scan.
///
/// The node keeps the awaiting flag and gets a [`ScanEvent::PermissionRequested`]
/// card, but its pending slot is released and it closes right away, so its
/// ancestors cascade: unauthorized directories never block the overall scan. A
/// later grant re-walks the subtree through the refresh path.
fn park_for_permission(
    state: &mut CoordinatorState,
    tree: &Arc<Mutex<ScanTree>>,
    index: u32,
    child_fds: ChildFdMap,
    tcc: bool,
) {
    let (key, path, name) = {
        let mut tree_guard = tree.lock().unwrap();
        let Some(node) = tree_guard.node_mut(index) else {
            // Node vanished; nothing to settle.
            return;
        };
        node.set_awaiting(true);
        let key = node.key;
        let path = tree_guard.path(index);
        let name = tree_guard.name_string(index);
        (key, path, name)
    };
    // Any preopened children (a read that failed after the directory itself was
    // opened) are closed: on grant the directory is walked again from scratch.
    drop(child_fds);

    state.awaiting.insert(
        key.to_string(),
        AwaitingDir {
            index,
            path: path.clone(),
            name: name.clone(),
        },
    );
    state.progress.awaiting += 1;

    let _ = state.event_tx.send(ScanEvent::PermissionRequested {
        scan: state.scan_id,
        key,
        path,
        name,
        tcc,
    });

    // Settle at zero now. The card above stays actionable — during the scan
    // and, via the lingering coordinator, after it completes.
    let mut tree_guard = tree.lock().unwrap();
    close_and_cascade(state, &mut tree_guard, index);
}

/// Decision key that skips every parked directory in one action.
pub const SKIP_ALL_KEY: &str = "*";

/// Mark every awaiting directory as explicitly skipped/denied.
///
/// Awaiting nodes were settled at zero when the scan passed them, so this only
/// flips their flags and publishes terminal `Denied` closes; nothing cascades
/// again.
fn skip_all_parked(state: &mut CoordinatorState, tree: &Arc<Mutex<ScanTree>>) {
    let all: Vec<AwaitingDir> =
        state.awaiting.drain().map(|(_, dir)| dir).collect();
    let count = all.len();
    state.progress.awaiting = 0;
    if count == 0 {
        return;
    }
    let mut tree_guard = tree.lock().unwrap();
    for waiting in all {
        state.progress.denied += 1;
        let key = if let Some(node) = tree_guard.node_mut(waiting.index) {
            node.set_awaiting(false);
            node.set_denied(true);
            node.key
        } else {
            continue;
        };
        let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
            scan: state.scan_id,
            key,
            status: DirCloseStatus::Denied,
        });
    }
    drop(tree_guard);
    let _ = state.event_tx.send(ScanEvent::Warning {
        scan: state.scan_id,
        message: format!("已批量跳过 {count} 个未授权目录"),
    });
}

/// Apply one user decision to a parked directory.
fn apply_permission_decision(
    state: &mut CoordinatorState,
    tree: &Arc<Mutex<ScanTree>>,
    decision: PermissionDecision,
) {
    // Skip-all: close every parked directory as denied.
    if decision.key == SKIP_ALL_KEY && !decision.granted {
        skip_all_parked(state, tree);
        return;
    }

    let Some(waiting) = state.awaiting.get(&decision.key).cloned() else {
        // Unknown / already decided key: ignore stale decisions.
        return;
    };
    let AwaitingDir { index, path, name } = waiting;

    let mut tree_guard = tree.lock().unwrap();
    if decision.granted {
        // Default: re-walk the parked directory itself. When the user selected
        // an ancestor folder (e.g. ~/Library for a denied Mail subdirectory),
        // re-walk that folder's existing subtree instead; every child node is
        // reused in place, including awaiting dirs reached along the way.
        let granted_root = decision
            .granted_path
            .filter(|selected| selected != &path && path_is_within(&path, selected))
            .and_then(|selected| index_by_path(&tree_guard, Path::new(&selected)));
        let root_index = granted_root.unwrap_or(index);

        // If the refresh root was itself awaiting, its decision is resolved
        // now. Other awaiting dirs below it are cleared when the walk reaches
        // them; the named entry itself follows the same path when it is not
        // the root.
        let root_awaiting_key = tree_guard
            .node(root_index)
            .filter(|node| node.awaiting())
            .map(|node| node.key.to_string());
        if let Some(key) = root_awaiting_key {
            state.awaiting.remove(&key);
            state.progress.awaiting = state.progress.awaiting.saturating_sub(1);
        }
        let root_path = tree_guard
            .path(root_index);
        start_refresh(state, &mut tree_guard, root_index, &root_path);
    } else {
        // Explicit skip. The node was settled at zero when the scan passed it,
        // so resolve its awaiting entry and mark it denied.
        state.awaiting.remove(&decision.key);
        state.progress.awaiting = state.progress.awaiting.saturating_sub(1);
        state.progress.denied += 1;
        let key = if let Some(node) = tree_guard.node_mut(index) {
            node.set_awaiting(false);
            node.set_denied(true);
            node.key
        } else {
            return;
        };
        let _ = state.event_tx.send(ScanEvent::Warning {
            scan: state.scan_id,
            message: format!("已跳过未授权目录: {name}"),
        });
        let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
            scan: state.scan_id,
            key,
            status: DirCloseStatus::Denied,
        });
    }
}

/// Re-open a settled awaiting node for a fresh walk of its subtree.
///
/// The node is marked pending again (its pending slot at the parent was
/// released at settle, so no parent slot is re-acquired — the refresh path
/// patches closed ancestors directly) and registered as a refresh root, so its
/// eventual close is routed through [`finish_refresh_root`].
fn start_refresh(
    state: &mut CoordinatorState,
    tree: &mut ScanTree,
    index: u32,
    path: &str,
) {
    if tree.node(index).is_some() {
        // Remove the old subtree totals (zero for an awaiting leaf, nonzero
        // when the user grants an ancestor that contains scanned dirs) from
        // the node and its closed ancestors before the walk rebuilds them.
        tree.prep_refresh(index);
    }
    if let Some(node) = tree.node_mut(index) {
        node.set_awaiting(false);
        node.set_pending(true);
    }
    tree.bump_hardlink_epoch();
    state.open_dirs += 1;
    state.refresh_roots.insert(index);
    state.refresh_open.insert(index);
    let seq = state.next_seq();
    let depth = tree.depth_of(index);
    let writable = DirReader::probe_writable(Path::new(path));
    state.queue.push(OrdDir {
        queued: QueuedDir {
            index,
            path: PathBuf::from(path),
            depth,
            seq,
            kind: JobKind::Tracked,
            writable,
        },
        category: category(Path::new(path), &state.focus),
        focus: state.focus.clone(),
        preopened: None,
    });
}

/// Whether `child` is `ancestor` itself or a directory nested inside it.
fn path_is_within(child: &str, ancestor: &str) -> bool {
    child == ancestor || child.starts_with(&format!("{ancestor}/"))
}

/// Find an existing child node of `parent` by raw entry name, building and
/// caching the parent's name→index map on first use. Only consulted inside
/// refresh walks, so normal discovery pays nothing.
fn existing_child(
    state: &mut CoordinatorState,
    tree: &ScanTree,
    parent: u32,
    name: &[u8],
) -> Option<u32> {
    if !state.existing_cache.contains_key(&parent) {
        let map = tree
            .children(parent)
            .into_iter()
            .map(|index| (tree.name(index).to_vec(), index))
            .collect();
        state.existing_cache.insert(parent, map);
    }
    state.existing_cache.get(&parent).and_then(|map| map.get(name)).copied()
}

/// Resolve an absolute path to its node index by walking the tree from the
/// root. Used only when a grant selected an ancestor folder.
fn index_by_path(tree: &ScanTree, target: &Path) -> Option<u32> {
    let root = tree.root();
    let mut text = target.to_string_lossy().to_string();
    if text != "/" {
        text = text.trim_end_matches('/').to_string();
    }
    if tree.root_path().to_string_lossy() == text {
        return Some(root);
    }
    let mut relative = text.strip_prefix(&tree.root_path().to_string_lossy().to_string())?;
    if let Some(stripped) = relative.strip_prefix('/') {
        relative = stripped;
    }
    let mut cursor = root;
    for component in relative.split('/').filter(|part| !part.is_empty()) {
        let next = tree
            .children(cursor)
            .into_iter()
            .find(|index| tree.name_string(*index) == component);
        cursor = next?;
    }
    Some(cursor)
}

fn process_dir_entries(
    state: &mut CoordinatorState,
    tree: &mut ScanTree,
    index: u32,
    dir_writable: bool,
    entries: &[RawEntry],
    mut child_fds: ChildFdMap,
) {
    let Some(dir) = tree.node(index) else {
        child_fds.close_remaining();
        return;
    };
    let dir_key = dir.key;
    let dir_path = tree.path(index);
    let parent_protected =
        sift_core::deletable::is_protected_location(Path::new(&dir_path));

    let mut child_dirs = 0u32;
    let mut dir_entries: Vec<DirectoryEntry> = Vec::with_capacity(entries.len());
    let mut newly_discovered: Vec<DiscoveredDir> = Vec::new();

    for entry in entries {
        if entry.name == b"." || entry.name == b".." {
            continue;
        }
        let child_key = dir_key.child(&entry.name, dir_path.ends_with('/'));
        let name_string = String::from_utf8_lossy(&entry.name).into_owned();
        let mtime_ms = entry.mtime_ms;
        let policy = sift_core::deletable::classify_child(&dir_path, &entry.name, parent_protected);
        let deletable = policy.is_yes() && dir_writable;

        if entry.is_dir && !entry.is_symlink {
            // Resume: this child was already restored into the tree.
            if state.adopted_set.contains(&child_key.to_string()) {
                let key_text = child_key.to_string();
                if state.adopted_closed.contains(&key_text) {
                    // Closed subtree: skip it entirely. Its aggregate already
                    // pre-charged the (open) parent at adoption, and the node is
                    // already closed, so do not recount or re-enqueue it.
                    state.progress.dirs += 1;
                    continue;
                }
                // Open adopted subtree: re-walk it at its existing node so its
                // unfinished contents are discovered. It was counted as pending
                // at adoption; that single pending slot is reused.
                if let Some(existing_index) = state.adopted_index.get(&key_text).copied() {
                    let preopened = child_fds.take(&entry.name);
                    let child_seq = state.next_seq();
                    state.queue.push(OrdDir {
                        queued: QueuedDir {
                            index: existing_index,
                            path: PathBuf::from(&join_bytes(&dir_path, &entry.name)),
                            depth: tree.depth_of(existing_index),
                            seq: child_seq,
                            kind: JobKind::Tracked,
                            writable: entry.writable_by_us(),
                        },
                        category: category(
                            Path::new(&join_bytes(&dir_path, &entry.name)),
                            &state.focus,
                        ),
                        focus: state.focus.clone(),
                        preopened,
                    });
                    child_dirs += 1;
                    state.progress.dirs += 1;
                }
                continue;
            }
            if state.root_dev != 0 && entry.dev != 0 && entry.dev != state.root_dev {
                // Another mounted filesystem (VM, Preboot, an external disk):
                // do not cross the boundary.
                state.progress.boundary_skipped += 1;
                continue;
            }
            let child_path = join_bytes(&dir_path, &entry.name);

            // Inside a refresh walk, reuse an existing child node — a normal
            // closed directory when an ancestor was granted, or an awaiting
            // leaf — so the subtree is never recorded twice. This must precede
            // the visited-dir dedup: a re-walked directory was visited in the
            // original walk and would otherwise be skipped as a duplicate.
            if state.refresh_open.contains(&index) {
                if let Some(existing) = existing_child(state, tree, index, &entry.name) {
                    let was_awaiting =
                        tree.node(existing).is_some_and(|node| node.awaiting());
                    if let Some(node) = tree.node_mut(existing) {
                        node.set_awaiting(false);
                        node.set_pending(true);
                        // Count the subtree from a clean base.
                        node.size = ByteSize::ZERO;
                        node.file_count = 0;
                    }
                    if was_awaiting {
                        state.awaiting.remove(&child_key.to_string());
                        state.progress.awaiting =
                            state.progress.awaiting.saturating_sub(1);
                    }
                    state.open_dirs += 1;
                    state.refresh_open.insert(existing);
                    let preopened = child_fds.take(&entry.name);
                    *state.pending_children.entry(index).or_insert(0) += 1;
                    let child_seq = state.next_seq();
                    state.queue.push(OrdDir {
                        queued: QueuedDir {
                            index: existing,
                            path: PathBuf::from(&child_path),
                            depth: tree.depth_of(existing),
                            seq: child_seq,
                            kind: JobKind::Tracked,
                            writable: entry.writable_by_us(),
                        },
                        category: category(Path::new(&child_path), &state.focus),
                        focus: state.focus.clone(),
                        preopened,
                    });
                    child_dirs += 1;
                    state.progress.dirs += 1;
                    dir_entries.push(DirectoryEntry::new(
                        child_key,
                        name_string,
                        true,
                        ByteSize::ZERO,
                        mtime_ms,
                        true,
                        deletable,
                    ));
                    continue;
                }
            }

            if entry.dev != 0
                && entry.ino != 0
                && !state.visited_dirs.insert((entry.dev, entry.ino))
            {
                // Already reached through another path (firmlink vs. mount
                // point): walk it only once.
                state.progress.boundary_skipped += 1;
                continue;
            }

            // Real directory: open in the tree, enqueue its read.
            let in_refresh = state.refresh_open.contains(&index);
            if let Some(child_ix) =
                tree.open_dir(index, child_key, &entry.name, mtime_ms, deletable, false)
            {
                state.open_dirs += 1;
                if in_refresh {
                    // The new node belongs to the refresh walk; record it so the
                    // cache knows the child now exists for later lookups.
                    state.refresh_open.insert(child_ix);
                    state
                        .existing_cache
                        .entry(index)
                        .or_default()
                        .insert(entry.name.clone(), child_ix);
                }
                let preopened = child_fds.take(&entry.name);
                let count = state.pending_children.entry(index).or_insert(0);
                *count += 1;
                // Giant detection happens in the worker via one `fstat` on
                // this preopened descriptor (directory size does not come back
                // in the bulk result on APFS).
                let child_seq = state.next_seq();
                state.queue.push(OrdDir {
                    queued: QueuedDir {
                        index: child_ix,
                        path: PathBuf::from(&child_path),
                        depth: tree.depth_of(child_ix),
                        seq: child_seq,
                        kind: JobKind::Tracked,
                        writable: entry.writable_by_us(),
                    },
                    category: category(Path::new(&child_path), &state.focus),
                    focus: state.focus.clone(),
                    preopened,
                });
                newly_discovered.push(DiscoveredDir::new(
                    child_key.to_string(),
                    dir_key.to_string(),
                    entry.name.clone(),
                    child_path.clone(),
                    tree.depth_of(child_ix),
                    mtime_ms,
                    entry.dev,
                    entry.ino,
                    entry.is_symlink,
                    deletable,
                ));
            } else {
                // Budget refused: still count the bytes via a size-only job.
                let count = state.size_only_pending.entry(index).or_insert(0);
                *count += 1;
                let size_seq = state.next_seq();
                state.queue.push(OrdDir {
                    queued: QueuedDir {
                        index,
                        path: PathBuf::from(&child_path),
                        depth: u16::MAX,
                        seq: size_seq,
                        kind: JobKind::SizeOnly,
                        writable: false,
                    },
                    category: 4,
                    focus: state.focus.clone(),
                    preopened: None,
                });
            }
            child_dirs += 1;
            dir_entries.push(DirectoryEntry::new(
                child_key,
                name_string,
                true,
                ByteSize::ZERO,
                mtime_ms,
                true,
                deletable,
            ));
        } else {
            // File (or symlink): count its bytes into the parent.
            //
            // Only a file with more than one link can be a hardlink duplicate, so
            // only those are offered to the dedupe table. A platform that does
            // not report a link count (`nlink == 0`) simply over-counts a
            // hardlinked file, which is the safe direction.
            let size = entry.size();
            let file_id = if entry.nlink > 1 {
                entry.file_id()
            } else {
                None
            };
            let counted = tree.record_file(index, size, file_id);
            if counted {
                state.progress.files += 1;
                state.progress.bytes += size;
                let retain_threshold = tree.config().file_detail_min_bytes;
                if size.logical >= retain_threshold {
                    tree.retain_file(index, child_key, &entry.name, size, mtime_ms, deletable);
                }
            } else {
                state.progress.hardlinks_skipped += 1;
            }
            dir_entries.push(DirectoryEntry::new(
                child_key,
                name_string,
                false,
                size,
                mtime_ms,
                false,
                deletable,
            ));
        }
    }

    if !newly_discovered.is_empty() {
        state.journal.discovered(&state.root_text, newly_discovered);
    }

    // Focus determinism: once the initial focus is listed (or found to be
    // unreachable), the coordinator may open the throttle to full parallelism.
    if !state.focus_processed {
        let initial_focus = state.initial_focus.to_string_lossy();
        if dir_path == initial_focus {
            state.focus_processed = true;
        } else if initial_focus.starts_with(&dir_path) {
            // This directory is an ancestor of the focus; if the next chain
            // component is not among its entries, the focus does not exist.
            let next = next_chain_component(&dir_path, &initial_focus);
            let reachable = entries
                .iter()
                .any(|e| e.is_dir && e.name == next.as_bytes());
            if !reachable {
                state.focus_processed = true;
            }
        }
    }

    child_fds.close_remaining();
    state.dirty.insert(index);

    // A directory with no subdirectories is complete the moment it is listed.
    if state.pending_children.get(&index).copied().unwrap_or(0) == 0
        && !state.size_only_pending.contains_key(&index)
    {
        close_and_cascade(state, tree, index);
    }

    let summary = {
        let node = tree.node(index);
        DirectorySummary::new(
            dir_key,
            dir_path,
            node.map(|n| n.size).unwrap_or_default(),
            node.map(|n| n.file_count).unwrap_or(0),
            child_dirs,
            dir_entries,
        )
    };
    let _ = state.event_tx.send(ScanEvent::DirectoryListed {
        scan: state.scan_id,
        dir: summary,
    });
}

/// Close `index` and cascade upward while parents become complete.
fn close_and_cascade(state: &mut CoordinatorState, tree: &mut ScanTree, index: u32) {
    let mut cursor = index;
    let mut newly_closed: Vec<ClosedDir> = Vec::new();
    loop {
        let Some(node) = tree.node(cursor) else {
            state.journal.closed(&state.root_text, newly_closed);
            return;
        };
        let parent = node.parent;
        let key = node.key;

        // Fold any unrecorded subtree bytes into this directory first.
        if let Some((size, files)) = state.unrecorded.remove(&cursor) {
            tree.add_unrecorded(cursor, size, files);
        }

        // A refresh root closes through the refresh path: its measured subtree
        // is re-credited to itself and its (closed) ancestors instead of being
        // rolled into the parent a second time or decrementing the pending slot
        // released when the node was first settled as awaiting.
        if state.refresh_roots.contains(&cursor) {
            finish_refresh_root(state, tree, cursor);
            state.journal.closed(&state.root_text, newly_closed);
            return;
        }

        let final_size = tree.node(cursor).map(|node| node.size).unwrap_or_default();
        let final_files = tree.node(cursor).map(|node| node.file_count).unwrap_or(0);
        let status = close_status(tree, cursor);

        tree.close_dir(cursor);
        state.open_dirs = state.open_dirs.saturating_sub(1);
        state.dirty.insert(cursor);
        state.progress.dirs += 1;
        newly_closed.push(ClosedDir::new(
            key.to_string(),
            final_size.logical,
            final_size.physical,
            final_files,
        ));

        let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
            scan: state.scan_id,
            key,
            status,
        });

        if parent == NONE {
            state.journal.closed(&state.root_text, newly_closed);
            return;
        }
        let remaining = {
            let entry = state.pending_children.entry(parent).or_insert(0);
            *entry = entry.saturating_sub(1);
            *entry
        };
        if remaining == 0 {
            state.pending_children.remove(&parent);
        }
        if remaining > 0 || state.size_only_pending.contains_key(&parent) {
            state.journal.closed(&state.root_text, newly_closed);
            return;
        }
        cursor = parent;
    }
}

/// How an ordinary close should be presented, from the node's current flags.
fn close_status(tree: &ScanTree, cursor: u32) -> DirCloseStatus {
    let Some(node) = tree.node(cursor) else {
        return DirCloseStatus::Ok;
    };
    if node.denied() {
        DirCloseStatus::Denied
    } else if node.awaiting() {
        DirCloseStatus::Awaiting
    } else {
        DirCloseStatus::Ok
    }
}

/// Finish the re-walk of a granted subtree at its root node.
///
/// The walk accumulated the subtree into `index` through normal
/// `record_file` / child closes, exactly as in a regular walk. That base is now
/// reset and credited once: to the node itself and to every ancestor up to the
/// first still open (its later close carries the bytes higher). Final sizes are
/// published for the root and each changed ancestor, then `RefreshFinished`
/// tells the front end the totals changed without a new full scan.
fn finish_refresh_root(state: &mut CoordinatorState, tree: &mut ScanTree, index: u32) {
    state.refresh_roots.remove(&index);

    let measured = tree.node(index).map(|node| node.size).unwrap_or_default();
    let files = tree.node(index).map(|node| node.file_count).unwrap_or(0);
    let key = tree
        .node(index)
        .map(|node| node.key)
        .unwrap_or_else(|| NodeKey::from_path(Path::new("")));
    let path = tree.path(index);

    // The root's base was zeroed by prep_refresh; the walk's accumulation in
    // `measured` is credited to its closed ancestors.
    let changed = tree.refresh_propagate(index, measured, files);

    state.open_dirs = state.open_dirs.saturating_sub(1);
    state.progress.dirs += 1;
    state.dirty.insert(index);

    let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
        scan: state.scan_id,
        key,
        status: DirCloseStatus::Ok,
    });
    let final_size = tree.node(index).map(|node| node.size).unwrap_or(measured);
    let _ = state.event_tx.send(ScanEvent::DirectorySized {
        scan: state.scan_id,
        key,
        size: final_size,
        files: tree.node(index).map(|node| node.file_count).unwrap_or(files),
        pending: false,
        estimated: false,
    });
    for ancestor in changed {
        let Some(node) = tree.node(ancestor) else { continue };
        let _ = state.event_tx.send(ScanEvent::DirectorySized {
            scan: state.scan_id,
            key: node.key,
            size: node.size,
            files: node.file_count,
            pending: node.pending(),
            estimated: node.estimated(),
        });
    }

    let _ = state.event_tx.send(ScanEvent::RefreshFinished {
        scan: state.scan_id,
        path,
    });
}

/// One predicted giant directory awaiting exact measurement.
struct CalibrationJob {
    index: u32,
    path: PathBuf,
}

/// A directory at or above this own-size threshold is treated as a giant.
///
/// On APFS a directory's `st_size` grows roughly linearly with its entry count
/// (~32 B/entry) once it spills out of inline storage, so 4 MiB predicts a
/// directory on the order of 100k+ entries.
const PREDICT_MIN_DIR_SIZE: u64 = 4 * 1024 * 1024;

/// Tag the node as an estimate and close it at its current (zero) totals.
fn close_predicted(state: &mut CoordinatorState, tree: &mut ScanTree, index: u32) {
    if let Some(node) = tree.node_mut(index) {
        node.set_estimated(true);
    }
    close_and_cascade(state, tree, index);
}

/// Measure every predicted giant subtree on a small thread pool, patching each
/// exact result into the closed tree and publishing `Calibrated`.
fn run_background_calibration(
    jobs: Vec<CalibrationJob>,
    tree: Arc<Mutex<ScanTree>>,
    event_tx: mpsc::Sender<ScanEvent>,
    scan_id: ScanId,
    workers: usize,
) {
    let started = Instant::now();
    let worker_count = workers.clamp(1, 16);
    let (job_tx, job_rx) = crossbeam_channel::unbounded::<CalibrationJob>();
    let (result_tx, result_rx) =
        mpsc::channel::<(u32, ByteSize, u32)>();

    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let job_rx = job_rx.clone();
        let result_tx = result_tx.clone();
        let handle = std::thread::Builder::new()
            .name("sift-calibrator".into())
            .spawn(move || {
                let mut buffer = BulkBuffer::new();
                while let Ok(job) = job_rx.recv() {
                    let (size, files) =
                        measure_subtree(&job.path, &mut buffer);
                    if result_tx.send((job.index, size, files)).is_err() {
                        break;
                    }
                }
            });
        if let Ok(handle) = handle {
            handles.push(handle);
        }
    }
    drop(result_tx);
    for job in jobs {
        if job_tx.send(job).is_err() {
            break;
        }
    }
    drop(job_tx);

    for (index, size, files) in result_rx {
        let (key, new_size, new_files) = {
            let mut guard = tree.lock().unwrap();
            let key = guard
                .node(index)
                .map(|node| node.key)
                .unwrap_or_else(|| NodeKey::from_path(Path::new("")));
            guard.adjust_totals(index, size, files as u64);
            if let Some(node) = guard.node_mut(index) {
                node.set_estimated(false);
            }
            let node = guard.node(index).copied();
            (
                key,
                node.map(|node| node.size).unwrap_or(size),
                node.map(|node| node.file_count as u64).unwrap_or(files as u64),
            )
        };
        let _ = event_tx.send(ScanEvent::Calibrated {
            scan: scan_id,
            key,
            size: new_size,
            files: new_files,
        });
    }

    for handle in handles {
        let _ = handle.join();
    }
    let _ = event_tx.send(ScanEvent::Warning {
        scan: scan_id,
        message: format!("大型目录校准完成（{:.1}s）", started.elapsed().as_secs_f64()),
    });
    let _ = event_tx.send(ScanEvent::CalibrationFinished { scan: scan_id });
}

/// Sum a whole subtree without building tree nodes, deduping hardlinks in a
/// local set the same way the tracked walk does. Iterative so stack usage is
/// bounded regardless of tree depth.
fn measure_subtree(path: &Path, buffer: &mut BulkBuffer) -> (ByteSize, u32) {
    let mut total = ByteSize::ZERO;
    let mut files = 0u32;
    let mut seen_links: HashSet<FileId> = HashSet::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = DirReader::read_reusing(&dir, true, buffer) else {
            continue;
        };
        for entry in entries {
            if entry.is_dir && !entry.is_symlink {
                stack.push(join_path(&dir, &entry.name));
            } else if entry.is_file {
                if entry.nlink > 1 {
                    if let Some(id) = entry.file_id() {
                        if !seen_links.insert(id) {
                            continue;
                        }
                    }
                }
                total += entry.size();
                files += 1;
            }
        }
    }
    (total, files)
}

/// Recompute every queued category against the current focus.
fn rebuild_categories(state: &mut CoordinatorState) {
    let items: Vec<OrdDir> = state.queue.drain().collect();
    for mut item in items {
        item.category = category(&item.queued.path, &state.focus);
        item.focus = state.focus.clone();
        state.queue.push(item);
    }
}

fn flush_progress(state: &mut CoordinatorState, tree: &Arc<Mutex<ScanTree>>) {
    state.progress.elapsed_secs = state.started.elapsed().as_secs_f64();
    state.progress.queue_depth = state.queue.len() as u64 + state.in_flight as u64;

    let tree_guard = tree.lock().unwrap();
    let total = tree_guard.total();

    // Byte coverage: how much of the volume's used space has been measured.
    let byte_coverage = match state.volume_used {
        Some(used) if used > 0 => {
            (state.progress.bytes.dominant() as f64 / used as f64).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    // Frontier coverage: fraction of known directories already closed. This
    // prevents a resumed scan (whose seeded bytes already equal the whole
    // volume) from showing 100% while its open frontier is still walking.
    let known_dirs = state.progress.dirs + state.open_dirs;
    let dir_coverage = if known_dirs > 0 {
        state.progress.dirs as f64 / known_dirs as f64
    } else {
        0.0
    };
    let mut coverage = byte_coverage.min(dir_coverage);
    if state.open_dirs > 0 {
        // 100% is reserved for a fully closed tree.
        coverage = coverage.min(0.99);
    } else {
        coverage = 1.0;
    }
    // Hold the peak: newly discovered subtrees enlarge the denominator and
    // would otherwise make the percent visibly move backwards.
    state.percent_peak = state.percent_peak.max(coverage);
    state.progress.coverage = state.percent_peak;

    // Interim size updates for directories whose totals changed.
    let dirty: Vec<u32> = state.dirty.iter().copied().collect();
    state.dirty.clear();
    for index in dirty {
        let Some(node) = tree_guard.node(index) else {
            continue;
        };
        let _ = state.event_tx.send(ScanEvent::DirectorySized {
            scan: state.scan_id,
            key: node.key,
            size: node.size,
            files: node.file_count,
            pending: node.pending(),
            estimated: node.estimated(),
        });
    }
    drop(tree_guard);
    let _ = total;

    let _ = state.event_tx.send(ScanEvent::Progress {
        scan: state.scan_id,
        progress: state.progress,
    });
}

impl CoordinatorState {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

// ---- worker-side reading ---------------------------------------------------

enum DirJob {
    Tracked {
        index: u32,
        path: PathBuf,
        /// Whether this directory is writable for us, already determined
        /// from the mode bits in its parent's bulk result (or by a one-time
        /// probe for the root). Workers never fstat for this.
        writable: bool,
        /// Descriptor inherited from the parent's read. `None` opens by path.
        preopened: Option<i32>,
    },
    SizeOnly { parent: u32, path: PathBuf },
}

/// Close an inherited descriptor if a dispatched job is ever dropped
/// undelivered (a drained channel on shutdown). Jobs destructured by a
/// worker move the descriptor out, so this does not double-close.
#[cfg(unix)]
impl Drop for DirJob {
    fn drop(&mut self) {
        if let DirJob::Tracked {
            preopened: Some(fd),
            ..
        } = self
        {
            close_raw_fd(*fd);
        }
    }
}

enum DirResult {
    Tracked {
        index: u32,
        writable: bool,
        entries: Result<Vec<RawEntry>, ReadErrorKind>,
        /// Preopened subdirectory descriptors, keyed by child name. Empty when
        /// the directory itself could not be opened.
        child_fds: ChildFdMap,
    },
    Sized {
        parent: u32,
        size: ByteSize,
        files: u32,
    },
    Predicted { index: u32 },
}

fn read_job(mut job: DirJob, want_physical: bool, buffer: &mut BulkBuffer) -> DirResult {
    match &mut job {
        DirJob::Tracked {
            index,
            path,
            writable,
            preopened,
        } => {
            // One fstat on the descriptor already in hand decides whether
            // this is a giant flat directory, which is skipped during the
            // main scan and measured exactly afterward. The scan root is
            // exempt: never predict the whole walk away.
            let index = *index;
            let writable = *writable;
            let mut preopened = preopened.take();
            let path = std::mem::take(path);
            let dir_size = match preopened {
                Some(fd) => fd_size(fd),
                None => path_size(&path),
            };
            if index != 0
                && dir_size.is_some_and(|size| size >= PREDICT_MIN_DIR_SIZE)
            {
                if let Some(fd) = preopened {
                    close_raw_fd(fd);
                }
                return DirResult::Predicted { index };
            }
            let outcome =
                read_with_children(preopened.take(), &path, want_physical, buffer);
            DirResult::Tracked {
                index,
                writable,
                entries: outcome.entries,
                child_fds: outcome.child_fds,
            }
        }
        DirJob::SizeOnly { parent, path } => {
            let parent = *parent;
            let path = std::mem::take(path);
            let (size, files) = size_only_walk(&path, want_physical, buffer);
            DirResult::Sized {
                parent,
                size,
                files,
            }
        }
    }
}

/// Sum a subtree without building any tree nodes. Iterative so worker stack
/// usage is bounded regardless of tree depth.
fn size_only_walk(
    path: &Path,
    want_physical: bool,
    buffer: &mut BulkBuffer,
) -> (ByteSize, u32) {
    let mut total = ByteSize::ZERO;
    let mut files = 0u32;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = DirReader::read_reusing(&dir, want_physical, buffer) else {
            continue;
        };
        for entry in entries {
            if entry.is_dir && !entry.is_symlink {
                stack.push(join_path(&dir, &entry.name));
            } else if entry.is_file {
                total += entry.size();
                files += 1;
            }
        }
    }
    (total, files)
}

// ---- path helpers ----------------------------------------------------------

/// The next path component of `focus` after `ancestor`, as a `String`, or an
/// empty string when `focus` is not under `ancestor`.
fn next_chain_component(ancestor: &str, focus: &str) -> String {
    let Some(rel) = focus.strip_prefix(ancestor) else {
        return String::new();
    };
    let rel = rel.trim_start_matches('/');
    let component = rel.split('/').next().unwrap_or("");
    component.to_string()
}

fn join_bytes(dir: &str, name: &[u8]) -> String {
    let mut out = String::with_capacity(dir.len() + name.len() + 1);
    out.push_str(dir);
    if !out.ends_with('/') {
        out.push('/');
    }
    out.push_str(&String::from_utf8_lossy(name));
    out
}

fn join_path(dir: &Path, name: &[u8]) -> PathBuf {
    let mut out = dir.to_path_buf();
    out.push(String::from_utf8_lossy(name).as_ref());
    out
}

/// Promote the calling thread to `USER_INITIATED` QoS on macOS so the scan
/// pool is scheduled onto the performance cores instead of sharing the
/// efficiency cluster. Rust-spawned threads otherwise inherit a default tier
/// that the scheduler is free to park on the low-power cores. Best effort:
/// any failure is ignored because priority is only a performance hint.
#[cfg(target_os = "macos")]
fn promote_thread_qos() {
    use std::ffi::c_void;
    use std::os::raw::c_int;

    // <sys/qos.h>. Directory walking is I/O-bound and runs alongside the
    // user's apps: default to the background-friendly UTILITY tier instead of
    // USER_INITIATED, so a full-volume scan never starves the foreground.
    // SIFT_SCAN_QOS=user restores the old behavior.
    const QOS_CLASS_UTILITY: u32 = 0x20;
    const QOS_CLASS_USER_INITIATED: u32 = 0x25;

    // `pthread_t` is an opaque pointer on macOS; declare just what we need so
    // this crate does not have to link libc directly.
    type PthreadT = *mut c_void;

    extern "C" {
        fn pthread_self() -> PthreadT;
        fn pthread_set_qos_class_np(
            thread: PthreadT,
            qos_class: u32,
            relative_priority: c_int,
        ) -> c_int;
    }

    let class = if std::env::var("SIFT_SCAN_QOS").as_deref() == Ok("user") {
        QOS_CLASS_USER_INITIATED
    } else {
        QOS_CLASS_UTILITY
    };
    unsafe {
        let _ = pthread_set_qos_class_np(pthread_self(), class, 0);
    }
}

/// Non-macOS: no thread QoS concept to tune.
#[cfg(not(target_os = "macos"))]
fn promote_thread_qos() {}

fn tree_config_for(policy: &ScanPolicy) -> TreeConfig {
    TreeConfig::from_scan_policy(policy)
}

/// The (device, inode) of the scan root, used to keep the walk on one volume
/// and to detect directories reached through more than one path. `(0, 0)`
/// when it cannot be read, which disables pruning rather than failing.
fn root_identity(root: &Path) -> (u64, u64) {
    sift_platform::volume::device_id(root)
        .map(|dev| {
            use std::os::unix::fs::MetadataExt;
            let ino = std::fs::metadata(root).map(|meta| meta.ino()).unwrap_or(0);
            (dev, ino)
        })
        .unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RestoredDir, RestoredScan};
    use sift_core::ScanId;
    use std::time::Duration;

    /// A unique temp tree per test: tests in one process share a pid, so a
    /// fixed name would let them delete each other's fixtures.
    fn temp_tree(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("sift-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b/c")).unwrap();
        std::fs::create_dir_all(base.join("x")).unwrap();
        std::fs::write(base.join("a/one.bin"), vec![b'1'; 100]).unwrap();
        std::fs::write(base.join("a/b/two.bin"), vec![b'2'; 200]).unwrap();
        std::fs::write(base.join("a/b/c/three.bin"), vec![b'3'; 300]).unwrap();
        std::fs::write(base.join("x/skip.bin"), vec![b'4'; 400]).unwrap();
        base
    }

    fn drain(handle: &ScanHandle) -> Vec<ScanEvent> {
        let mut events = Vec::new();
        while let Ok(event) = handle.events.recv() {
            let is_finished = matches!(event, ScanEvent::Finished { .. });
            events.push(event);
            if is_finished {
                break;
            }
        }
        events
    }

    #[test]
    fn scans_a_tree_and_rolls_sizes_up() {
        let base = temp_tree("rollup");
        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(
                ScanRequest::new(ScanId(1), base.clone())
                    .with_focus(base.join("a"))
                    .with_volume_used_bytes(1000),
            )
            .unwrap();
        let events = drain(&handle);
        handle.join();

        // A DirectoryListed for the root and for "a".
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::DirectoryListed { dir, .. } if dir.path == base.to_string_lossy())));
        assert!(events.iter().any(|e| matches!(
            e,
            ScanEvent::DirectoryListed { dir, .. } if dir.path == base.join("a").to_string_lossy()
        )));

        // Final totals: 100 + 200 + 300 + 400 = 1000 logical bytes.
        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, 1000);
        assert_eq!(tree.node(0).unwrap().file_count, 4);
        drop(tree);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn focus_directory_beats_non_ancestors() {
        let base = temp_tree("focus");
        let engine = ScanEngine::with_workers(4);
        let focus = base.join("a");
        let handle = engine
            .scan(ScanRequest::new(ScanId(2), base.clone()).with_focus(focus.clone()))
            .unwrap();

        // Tree-building walks open a parent before its children, so the root is
        // necessarily listed first. The priority contract is that the focus
        // directory is published before any sibling or unrelated subtree.
        let mut focus_index: Option<usize> = None;
        let mut sibling_index: Option<usize> = None;
        let mut listed: Vec<String> = Vec::new();
        loop {
            match handle.events.recv() {
                Ok(ScanEvent::DirectoryListed { dir, .. }) => {
                    if dir.path == focus.to_string_lossy() {
                        focus_index = Some(listed.len());
                    }
                    // The "x" directory is a sibling of focus; it must lose.
                    if dir.path == base.join("x").to_string_lossy() {
                        sibling_index = Some(listed.len());
                    }
                    listed.push(dir.path.clone());
                }
                Ok(ScanEvent::Finished { .. }) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        let focus_ix = focus_index.expect("focus directory was listed");
        if let Some(sib_ix) = sibling_index {
            assert!(
                focus_ix < sib_ix,
                "focus ({focus_ix}) must be listed before sibling ({sib_ix}); order: {listed:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unreadable_directory_is_denied_not_fatal() {
        let base = temp_tree("denied");
        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(ScanRequest::new(ScanId(3), base.clone()))
            .unwrap();
        let events = drain(&handle);
        handle.join();

        let finished = events
            .iter()
            .find_map(|e| match e {
                ScanEvent::Finished { outcome, .. } => Some(*outcome),
                _ => None,
            })
            .expect("Finished event");
        assert_eq!(finished, ScanOutcome::Completed);
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::Progress { progress, .. } if progress.dirs >= 3)));

        let _ = std::fs::remove_dir_all(&base);
    }

    extern "C" {
        fn geteuid() -> u32;
    }

    /// chmod-based permission tests are meaningless for root, which bypasses
    /// mode bits.
    fn running_as_root() -> bool {
        unsafe { geteuid() == 0 }
    }

    /// Pull events until a `PermissionRequested` arrives (or the scan ends /
    /// ten seconds elapse).
    fn next_permission_request(handle: &ScanHandle) -> Option<(NodeKey, String)> {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            match handle
                .events
                .recv_timeout(std::time::Duration::from_millis(200))
            {
                Ok(ScanEvent::PermissionRequested { key, path, .. }) => {
                    return Some((key, path));
                }
                Ok(ScanEvent::Finished { .. }) => return None,
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        None
    }

    /// Drain events until `Finished`, with an overall timeout. Returns whether
    /// it arrived.
    fn wait_for_finished(handle: &ScanHandle) -> bool {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match handle.events.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(ScanEvent::Finished { .. }) => return true,
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    /// Drain events until a post-completion refresh finishes.
    fn wait_for_refresh(handle: &ScanHandle) -> bool {
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match handle.events.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(ScanEvent::RefreshFinished { .. }) => return true,
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn scan_completes_with_awaiting_dir_then_grant_rewalks_it() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return;
        }

        let base = temp_tree("permgrant");
        let locked = base.join("x");
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked, perms).unwrap();

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(ScanRequest::new(ScanId(10), base.clone()))
            .unwrap();
        let (key, path) = next_permission_request(&handle)
            .expect("an unreadable directory must request permission");
        assert_eq!(path, locked.to_string_lossy());

        // The scan completes despite the unresolved directory: it was settled
        // at zero so the tree fully closes.
        assert!(wait_for_finished(&handle), "the scan must not wait for authorization");
        assert_eq!(
            handle.tree.lock().unwrap().total().logical,
            600,
            "the awaiting subtree contributes nothing"
        );

        // User grants after completion: restore access and send the decision.
        // The directory is refresh-walked and its 400 bytes patch the totals.
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&locked, perms).unwrap();
        handle
            .control()
            .resolve_permission(key.to_string(), true, None)
            .unwrap();
        assert!(wait_for_refresh(&handle), "the grant must finish a refresh walk");

        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, 1000, "x's 400 bytes counted after grant");
        assert_eq!(
            tree.node(0).unwrap().size.logical, 1000,
            "the refreshed subtree reaches the root"
        );
        drop(tree);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_skip_marks_awaiting_directory_denied() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return;
        }

        let base = temp_tree("permskip");
        let locked = base.join("x");
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked, perms).unwrap();

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(ScanRequest::new(ScanId(11), base.clone()))
            .unwrap();
        let (key, path) = next_permission_request(&handle)
            .expect("an unreadable directory must request permission");
        assert_eq!(path, locked.to_string_lossy());

        // Explicit skip: the already-settled node is marked denied; the scan
        // completes without the x subtree.
        handle
            .control()
            .resolve_permission(key.to_string(), false, None)
            .unwrap();
        let events = drain(&handle);

        assert!(events.iter().any(|e| matches!(
            e,
            ScanEvent::Finished {
                outcome: ScanOutcome::Completed,
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ScanEvent::Warning { message, .. } if message.contains("x")
        )));
        let denied = events.iter().find_map(|e| match e {
            ScanEvent::Finished { progress, .. } => Some(progress.denied),
            _ => None,
        });
        assert!(denied.unwrap_or(0) >= 1, "the skip is counted as denied: {denied:?}");
        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, 600, "the skipped subtree contributes nothing");
        drop(tree);

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&locked, perms).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_grant_rewalks_every_awaiting_directory_inside() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return;
        }

        let base = std::env::temp_dir()
            .join(format!("sift-scan-fanout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("x")).unwrap();
        std::fs::create_dir_all(base.join("y")).unwrap();
        std::fs::write(base.join("x/xb.bin"), vec![b'x'; 400]).unwrap();
        std::fs::write(base.join("y/yb.bin"), vec![b'y'; 700]).unwrap();

        let lock = |path: &Path| {
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o000);
            std::fs::set_permissions(path, perms).unwrap();
        };
        lock(&base.join("x"));
        lock(&base.join("y"));

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(
                ScanRequest::new(ScanId(20), base.clone())
                    .with_volume_used_bytes(1100),
            )
            .unwrap();

        // Both siblings settle as awaiting before any decision.
        let (first_key, _) = next_permission_request(&handle)
            .expect("an unreadable directory must request permission");
        let mut awaiting_seen = 0u64;
        let mut finished_seen = false;
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        while Instant::now() < deadline {
            match handle
                .events
                .recv_timeout(std::time::Duration::from_millis(200))
            {
                Ok(ScanEvent::Progress { progress, .. }) => {
                    awaiting_seen = awaiting_seen.max(progress.awaiting);
                    if awaiting_seen >= 2 {
                        break;
                    }
                }
                Ok(ScanEvent::Finished { .. }) => {
                    finished_seen = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(awaiting_seen >= 2, "both locked dirs must await: {awaiting_seen}");

        // The scan finishes with both dirs unresolved.
        if !finished_seen {
            assert!(wait_for_finished(&handle), "the scan must complete on its own");
        }
        assert_eq!(
            handle.tree.lock().unwrap().total().logical,
            0,
            "nothing counted while both subtrees await"
        );

        // User unlocks both and grants the parent folder. A single decision
        // refresh-walks the root, reusing both awaiting nodes in place.
        let unlock = |path: &Path| {
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        };
        unlock(&base.join("x"));
        unlock(&base.join("y"));
        handle
            .control()
            .resolve_permission(first_key.to_string(), true, Some(base.to_string_lossy().into()))
            .unwrap();
        assert!(wait_for_refresh(&handle), "the ancestor grant must finish a refresh walk");

        let tree = handle.tree.lock().unwrap();
        assert_eq!(
            tree.total().logical, 1100,
            "both subtrees counted after one ancestor grant"
        );
        drop(tree);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn awaiting_directory_completes_at_100_then_grant_stays_at_100() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return;
        }

        let base = temp_tree("pctpark");
        let locked = base.join("x");
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked, perms).unwrap();

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(
                ScanRequest::new(ScanId(21), base.clone())
                    // Accessible bytes (600) equal the declared volume usage.
                    // The unresolved dir no longer holds the tree open.
                    .with_volume_used_bytes(600),
            )
            .unwrap();
        let (key, _) = next_permission_request(&handle)
            .expect("an unreadable directory must request permission");

        // Scan completes with the tree fully closed: coverage reaches 100%
        // even though the x subtree is unresolved.
        let events = drain(&handle);
        let final_coverage = events.iter().rev().find_map(|e| match e {
            ScanEvent::Finished { progress, .. } => Some(progress.coverage),
            _ => None,
        });
        assert_eq!(final_coverage, Some(1.0), "a closed tree reports 100%");

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&locked, perms).unwrap();
        handle
            .control()
            .resolve_permission(key.to_string(), true, None)
            .unwrap();
        assert!(wait_for_refresh(&handle), "the grant must finish a refresh walk");

        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, 1000);
        assert_eq!(tree.node(0).unwrap().size.logical, 1000);
        drop(tree);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_with_partial_results() {
        let base = temp_tree("cancel");
        let engine = ScanEngine::with_workers(4);
        let handle = engine
            .scan(ScanRequest::new(ScanId(4), base.clone()))
            .unwrap();
        // Cancel quickly; the coordinator must emit Cancelled eventually.
        handle.cancel();
        let events = drain(&handle);
        handle.join();
        let outcome = events.iter().find_map(|e| match e {
            ScanEvent::Finished { outcome, .. } => Some(*outcome),
            _ => None,
        });
        assert_eq!(outcome, Some(ScanOutcome::Cancelled));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Hardlinked files must be counted once, through the real walk: the
    /// platform reports the link count, the engine only offers multiply-linked
    /// files to the dedupe table, and the tree sums the bytes once.
    #[cfg(unix)]
    #[test]
    fn a_hardlinked_file_is_counted_once_end_to_end() {
        let base = temp_tree("hardlink");
        // Two names, one inode, 4096 bytes of payload.
        let original = base.join("a/original.bin");
        std::fs::write(&original, vec![3u8; 4096]).unwrap();
        std::fs::hard_link(&original, base.join("a/linked.bin")).unwrap();
        let extra = base.join("x/other.bin");
        std::fs::write(&extra, vec![4u8; 1024]).unwrap();

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(ScanRequest::new(ScanId(9), base.clone()).with_policy(ScanPolicy::thorough()))
            .unwrap();
        let events = drain(&handle);
        handle.join();

        let progress = events
            .iter()
            .find_map(|event| match event {
                ScanEvent::Finished { progress, .. } => Some(*progress),
                _ => None,
            })
            .expect("a Finished event");

        // The fixture's files (100 + 200 + 300 + 400) plus 4096 for the hardlink
        // pair and 1024 for the extra file. Were the second name counted too, the
        // total would be 10216.
        let expected = 100 + 200 + 300 + 400 + 4096 + 1024;
        assert_eq!(
            expected, 6120,
            "the fixture arithmetic is the point of the test"
        );
        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, expected);
        assert!(
            progress.hardlinks_skipped >= 1,
            "the second name of a hardlinked file must be skipped, got {progress:?}"
        );
        assert_eq!(tree.stats().hardlink_entries, 1, "one inode remembered");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pause_holds_completion_until_resume() {
        // A broad tree (many leaf dirs) so a scan is still in flight when we
        // pause immediately after `scan()` returns.
        let tag = "pause";
        let base = std::env::temp_dir().join(format!("sift-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for i in 0..60 {
            let dir = base.join(format!("d{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("f.bin"), vec![b'z'; 4000]).unwrap();
        }

        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan(ScanRequest::new(ScanId(6), base.clone()))
            .unwrap();
        let control = handle.control();
        control.pause();
        assert!(control.is_paused());

        // Give the in-flight jobs time to settle; the scan must NOT finish.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !handle.finished.load(AtomicOrdering::SeqCst),
            "a paused scan must never complete"
        );

        control.resume();
        let events = drain(&handle);
        handle.join();
        assert!(events.iter().any(|e| matches!(
            e,
            ScanEvent::Finished {
                outcome: ScanOutcome::Completed,
                ..
            }
        )));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resume_adopts_closed_subtree_and_rewalks_open_frontier() {
        use std::os::unix::fs::MetadataExt;

        let base = temp_tree("resume");
        let journal = TestJournal::default();
        let (root_dev, root_ino) = root_identity(&base);
        let ino_of = |path: &Path| std::fs::metadata(path).unwrap().ino();

        // Simulate the state an interrupted scan persisted:
        //  - root discovered
        //  - `a` fully closed (its 100+200+300 = 600 logical bytes, 3 files)
        //  - `x` discovered but not yet closed (the unfinished frontier)
        let root_key_text = NodeKey::from_path(&base).to_string();
        let a_path = base.join("a");
        let x_path = base.join("x");
        let a_key_text = NodeKey::from_path(&a_path).to_string();
        let x_key_text = NodeKey::from_path(&x_path).to_string();
        let root_text = base.to_string_lossy().to_string();

        let root_dir = DiscoveredDir::new(
            root_key_text.clone(),
            String::new(),
            base.file_name().unwrap().as_encoded_bytes().to_vec(),
            root_text.clone(),
            0,
            0,
            root_dev,
            root_ino,
            false,
            false,
        );
        journal.begin(&root_dir);

        let mk = |path: &Path, key: String, name: &str, depth: u16| {
            DiscoveredDir::new(
                key,
                root_key_text.clone(),
                name.as_bytes().to_vec(),
                path.to_string_lossy().to_string(),
                depth,
                std::fs::metadata(path).unwrap()
                    .mtime()
                    .saturating_mul(1000),
                root_dev,
                ino_of(path),
                false,
                false,
            )
        };
        let a_dir = mk(&a_path, a_key_text.clone(), "a", 1);
        let x_dir = mk(&x_path, x_key_text.clone(), "x", 1);
        journal.discovered(&root_text, vec![a_dir, x_dir]);
        journal.closed(
            &root_text,
            vec![ClosedDir::new(a_key_text.clone(), 600, 600, 3)],
        );

        // Resume. Closed `a` is skipped; open `x` is re-walked (400 bytes).
        let engine = ScanEngine::with_workers(2);
        let handle = engine
            .scan_with_journal(
                ScanRequest::new(ScanId(8), base.clone()),
                Arc::new(journal),
            )
            .unwrap();
        let events = drain(&handle);
        handle.join();

        let outcome = events.iter().find_map(|e| match e {
            ScanEvent::Finished { outcome, .. } => Some(*outcome),
            _ => None,
        });
        assert_eq!(outcome, Some(ScanOutcome::Completed));
        let tree = handle.tree.lock().unwrap();
        assert_eq!(tree.total().logical, 1000, "600 adopted + 400 re-walked");
        assert_eq!(tree.node(0).unwrap().file_count, 4);
        drop(tree);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Synchronous in-memory journal used by the resume test.
    #[derive(Default)]
    struct TestJournal {
        discovered: Mutex<HashMap<String, Vec<DiscoveredDir>>>,
        closed: Mutex<HashMap<String, Vec<ClosedDir>>>,
    }

    impl ScanJournal for TestJournal {
        fn begin(&self, root: &DiscoveredDir) {
            self.discovered
                .lock()
                .unwrap()
                .entry(root.path.clone())
                .or_default()
                .push(root.clone());
        }

        fn discovered(&self, scan_root: &str, dirs: Vec<DiscoveredDir>) {
            self.discovered
                .lock()
                .unwrap()
                .entry(scan_root.to_string())
                .or_default()
                .extend(dirs);
        }

        fn closed(&self, scan_root: &str, updates: Vec<ClosedDir>) {
            self.closed
                .lock()
                .unwrap()
                .entry(scan_root.to_string())
                .or_default()
                .extend(updates);
        }

        fn load(&self, root: &str) -> RestoredScan {
            let mut dirs: Vec<RestoredDir> = self
                .discovered
                .lock()
                .unwrap()
                .get(root)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|d| {
                    RestoredDir::new(d, false, 0, 0, 0)
                })
                .collect();
            let closed = self.closed.lock().unwrap();
            if let Some(updates) = closed.get(root) {
                for update in updates {
                    if let Some(slot) = dirs
                        .iter_mut()
                        .find(|d| d.discovered.key == update.key)
                    {
                        *slot = RestoredDir::new(
                            slot.discovered.clone(),
                            true,
                            update.logical,
                            update.physical,
                            update.files,
                        );
                    }
                }
            }
            dirs.sort_by_key(|d| d.discovered.depth);
            RestoredScan::new(dirs)
        }

        fn complete(&self, root: &str) {
            self.discovered.lock().unwrap().remove(root);
            self.closed.lock().unwrap().remove(root);
        }
    }
}
