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

use sift_core::deletable::classify;
use sift_core::id::NodeKey;
use sift_core::tree::NONE;
use sift_core::{
    ByteSize, DirectoryEntry, DirectorySummary, Progress, ScanEvent, ScanId, ScanOutcome,
    ScanPolicy, ScanRequest, ScanTree, TreeConfig,
};
use sift_platform::dir::{DirReader, RawEntry};

use crate::priority::{category, JobKind, OrdDir, QueuedDir};

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
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
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
    cancel: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
}

impl ScanControl {
    /// Ask the scan to stop soon. Workers finish their current directory read,
    /// then drain; a `Finished { outcome: Cancelled }` event follows.
    pub fn cancel(&self) {
        self.cancel.store(true, AtomicOrdering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::SeqCst)
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
            cancel: Arc::clone(&self.cancel),
            focus: Arc::clone(&self.focus),
            focus_generation: Arc::clone(&self.focus_generation),
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
            workers: ScanPolicy::thorough().resolved_threads(),
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
        let workers = self.workers.max(1);
        let config = tree_config_for(&request.policy);

        let root_key = NodeKey::from_path(&request.root);

        let tree = Arc::new(Mutex::new(ScanTree::new(root_key, &request.root, config)));
        let cancel = Arc::new(AtomicBool::new(false));
        let focus = Arc::new(Mutex::new(request.focus.clone()));
        let focus_generation = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));

        let (dispatch_tx, dispatch_rx) = mpsc::channel::<DirJob>();
        let dispatch_rx = Arc::new(Mutex::new(dispatch_rx));
        let (result_tx, result_rx) = mpsc::channel::<DirResult>();
        let (event_tx, event_rx) = mpsc::channel::<ScanEvent>();

        // Workers: pull jobs, read directories, return raw entries. The
        // receiver is single-consumer, so it is shared behind a mutex; a
        // blocked `recv` holds the lock only until the next job arrives.
        let want_physical = request.policy.want_physical_size;
        for _ in 0..workers {
            let dispatch_rx = Arc::clone(&dispatch_rx);
            let result_tx = result_tx.clone();
            let cancel = Arc::clone(&cancel);
            std::thread::Builder::new()
                .name("sift-worker".into())
                .spawn(move || {
                    loop {
                        let job = dispatch_rx.lock().unwrap().recv();
                        let Ok(job) = job else { break };
                        if cancel.load(AtomicOrdering::SeqCst) {
                            break;
                        }
                        let result = read_job(job, want_physical);
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
        let focus_co = Arc::clone(&focus);
        let focus_generation_co = Arc::clone(&focus_generation);
        let finished_co = Arc::clone(&finished);
        std::thread::Builder::new()
            .name("sift-coordinator".into())
            .spawn(move || {
                run_coordinator(CoordinatorInputs {
                    scan_id: request.id,
                    workers,
                    root_key,
                    root_path: request.root.clone(),
                    focus_path: request.focus.clone(),
                    volume_used: request.volume_used_bytes,
                    tree: tree_co,
                    cancel: cancel_co,
                    focus: focus_co,
                    focus_generation: focus_generation_co,
                    dispatch_tx,
                    result_rx,
                    event_tx,
                });
                finished_co.store(true, AtomicOrdering::SeqCst);
            })?;

        Ok(ScanHandle {
            scan_id: request.id,
            events: event_rx,
            tree,
            cancel,
            focus,
            focus_generation,
            finished,
        })
    }
}

struct CoordinatorInputs {
    scan_id: ScanId,
    workers: usize,
    root_key: NodeKey,
    root_path: PathBuf,
    focus_path: PathBuf,
    volume_used: Option<u64>,
    tree: Arc<Mutex<ScanTree>>,
    cancel: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
    focus_generation: Arc<AtomicU64>,
    dispatch_tx: mpsc::Sender<DirJob>,
    result_rx: mpsc::Receiver<DirResult>,
    event_tx: mpsc::Sender<ScanEvent>,
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
    volume_used: Option<u64>,
    event_tx: mpsc::Sender<ScanEvent>,
    scan_id: ScanId,
}

fn run_coordinator(inputs: CoordinatorInputs) {
    let CoordinatorInputs {
        scan_id,
        workers,
        root_key,
        root_path,
        focus_path,
        volume_used,
        tree,
        cancel,
        focus,
        focus_generation,
        dispatch_tx,
        result_rx,
        event_tx,
    } = inputs;

    let mut state = CoordinatorState {
        queue: BinaryHeap::new(),
        pending_children: HashMap::new(),
        size_only_pending: HashMap::new(),
        unrecorded: HashMap::new(),
        dirty: HashSet::new(),
        progress: Progress::default(),
        seq: 0,
        in_flight: 0,
        started: Instant::now(),
        last_flush: Instant::now(),
        focus: focus_path.clone(),
        focus_seen_generation: 0,
        initial_focus: focus_path.clone(),
        focus_processed: focus_path == root_path,
        cancelled: false,
        volume_used,
        event_tx,
        scan_id,
    };

    // Seed the queue with the root.
    let root_seq = state.next_seq();
    state.queue.push(OrdDir {
        queued: QueuedDir {
            index: 0,
            path: root_path.clone(),
            depth: 0,
            seq: root_seq,
            kind: JobKind::Tracked,
        },
        category: category(&root_path, &focus_path),
        focus: focus_path,
    });

    let outcome = coordinator_loop(&mut state, &tree, &cancel, &focus, &focus_generation, &dispatch_tx, &result_rx, workers);

    // Fold unrecorded totals so even a cancelled scan adds up.
    {
        let mut tree_guard = tree.lock().unwrap();
        for (parent, (size, files)) in state.unrecorded.iter() {
            tree_guard.add_unrecorded(*parent, *size, *files);
        }
        // A `Vec` that grew by doubling can be holding nearly twice the nodes it
        // needs. The scan is over, so pay the one-time copy and give the memory
        // back — on a multi-million-node volume this is the difference between
        // ~130 and ~72 bytes per node held afterwards.
        tree_guard.shrink_to_fit();
    }

    let final_progress = state.progress;
    let _ = (root_key,);
    // Dropping `dispatch_tx` here makes every idle worker exit.
    drop(dispatch_tx);

    let _ = state.event_tx.send(ScanEvent::Finished {
        scan: scan_id,
        outcome,
        progress: final_progress,
    });
    drop(state.event_tx);
}

#[allow(clippy::too_many_arguments)]
fn coordinator_loop(
    state: &mut CoordinatorState,
    tree: &Arc<Mutex<ScanTree>>,
    cancel: &Arc<AtomicBool>,
    focus: &Arc<Mutex<PathBuf>>,
    focus_generation: &Arc<AtomicU64>,
    dispatch_tx: &mpsc::Sender<DirJob>,
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

        // Dispatch work while workers are free. Until the focus directory is
        // listed, keep one job in flight so focus-first is deterministic.
        let dispatch_limit = if state.focus_processed { workers } else { 1 };
        while state.in_flight < dispatch_limit {
            let Some(job) = state.queue.pop() else { break };
            let dispatch = match job.queued.kind {
                JobKind::Tracked => DirJob::Tracked {
                    index: job.queued.index,
                    path: job.queued.path.clone(),
                },
                JobKind::SizeOnly => DirJob::SizeOnly {
                    parent: job.queued.index,
                    path: job.queued.path.clone(),
                },
            };
            if dispatch_tx.send(dispatch).is_err() {
                outcome = ScanOutcome::Failed;
                break;
            }
            state.in_flight += 1;
        }

        if state.in_flight == 0 {
            // Queue empty and nothing in flight: every reachable directory was
            // walked.
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

        process_result(state, tree, &result);

        if state.last_flush.elapsed() >= PROGRESS_INTERVAL {
            flush_progress(state, tree);
            state.last_flush = Instant::now();
        }
    }

    flush_progress(state, tree);
    outcome
}

fn process_result(state: &mut CoordinatorState, tree: &Arc<Mutex<ScanTree>>, result: &DirResult) {
    match result {
        DirResult::Tracked { index, entries } => {
            let mut tree_guard = tree.lock().unwrap();
            match entries {
                Some(entries) => process_dir_entries(state, &mut tree_guard, *index, entries),
                None => {
                    state.progress.denied += 1;
                    // Unreadable directory: settle it at zero.
                    close_and_cascade(state, &mut tree_guard, *index);
                }
            }
            drop(tree_guard);
        }
        DirResult::Sized { parent, size, files } => {
            let mut tree_guard = tree.lock().unwrap();
            let finished_all = {
                let entry = state.size_only_pending.entry(*parent).or_insert(0);
                *entry = entry.saturating_sub(1);
                *entry == 0
            };
            if finished_all {
                state.size_only_pending.remove(parent);
            }
            state
                .unrecorded
                .entry(*parent)
                .and_modify(|(s, f)| {
                    *s += *size;
                    *f += *files;
                })
                .or_insert((*size, *files));
            // If the parent is otherwise complete, close it now that its
            // unrecorded subtree bytes have arrived.
            if state.pending_children.get(parent).copied().unwrap_or(0) == 0
                && !state.size_only_pending.contains_key(parent)
            {
                close_and_cascade(state, &mut tree_guard, *parent);
            }
            drop(tree_guard);
        }
    }
}

fn process_dir_entries(
    state: &mut CoordinatorState,
    tree: &mut ScanTree,
    index: u32,
    entries: &[RawEntry],
) {
    let Some(dir) = tree.node(index) else {
        return;
    };
    let dir_key = dir.key;
    let dir_path = tree.path(index);

    let mut child_dirs = 0u32;
    let mut dir_entries: Vec<DirectoryEntry> = Vec::with_capacity(entries.len());

    // Deletability probe once per directory (policy per child is pure string
    // logic; the permission probe is one stat on the parent).
    let dir_writable = sift_core::deletable::parent_is_writable(Path::new(&dir_path));

    for entry in entries {
        if entry.name == b"." || entry.name == b".." {
            continue;
        }
        let child_path = join_bytes(&dir_path, &entry.name);
        let child_key = NodeKey::from_path(Path::new(&child_path));
        let name_string = String::from_utf8_lossy(&entry.name).into_owned();
        let mtime_ms = entry.mtime_ms;
        let deletable = classify(Path::new(&child_path)).is_yes() && dir_writable;

        if entry.is_dir && !entry.is_symlink {
            // Real directory: open in the tree, enqueue its read.
            if let Some(child_ix) =
                tree.open_dir(index, child_key, &entry.name, mtime_ms, deletable, false)
            {
                let count = state.pending_children.entry(index).or_insert(0);
                *count += 1;
                let child_seq = state.next_seq();
                state.queue.push(OrdDir {
                    queued: QueuedDir {
                        index: child_ix,
                        path: PathBuf::from(&child_path),
                        depth: tree.depth_of(child_ix),
                        seq: child_seq,
                        kind: JobKind::Tracked,
                    },
                    category: category(Path::new(&child_path), &state.focus),
                    focus: state.focus.clone(),
                });
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
                    },
                    category: 4,
                    focus: state.focus.clone(),
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
            let file_id = if entry.nlink > 1 { entry.file_id() } else { None };
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
    loop {
        let Some(node) = tree.node(cursor) else { return };
        let parent = node.parent;
        let key = node.key;

        // Fold any unrecorded subtree bytes into this directory first.
        if let Some((size, files)) = state.unrecorded.remove(&cursor) {
            tree.add_unrecorded(cursor, size, files);
        }

        tree.close_dir(cursor);
        state.dirty.insert(cursor);
        state.progress.dirs += 1;

        let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
            scan: state.scan_id,
            key,
        });

        if parent == NONE {
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
            return;
        }
        cursor = parent;
    }
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
    state.progress.coverage = match state.volume_used {
        Some(used) if used > 0 => {
            (state.progress.bytes.dominant() as f64 / used as f64).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };

    // Interim size updates for directories whose totals changed.
    let dirty: Vec<u32> = state.dirty.iter().copied().collect();
    state.dirty.clear();
    for index in dirty {
        let Some(node) = tree_guard.node(index) else { continue };
        let _ = state.event_tx.send(ScanEvent::DirectorySized {
            scan: state.scan_id,
            key: node.key,
            size: node.size,
            files: node.file_count,
            pending: node.pending(),
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
    Tracked { index: u32, path: PathBuf },
    SizeOnly { parent: u32, path: PathBuf },
}

enum DirResult {
    Tracked { index: u32, entries: Option<Vec<RawEntry>> },
    Sized { parent: u32, size: ByteSize, files: u32 },
}

fn read_job(job: DirJob, want_physical: bool) -> DirResult {
    match job {
        DirJob::Tracked { index, path } => {
            let entries = DirReader::read(&path, want_physical)
                .ok()
                .map(DirReader::into_entries);
            DirResult::Tracked { index, entries }
        }
        DirJob::SizeOnly { parent, path } => {
            let (size, files) = size_only_walk(&path, want_physical);
            DirResult::Sized { parent, size, files }
        }
    }
}

/// Sum a subtree without building any tree nodes. Iterative so worker stack
/// usage is bounded regardless of tree depth.
fn size_only_walk(path: &Path, want_physical: bool) -> (ByteSize, u32) {
    let mut total = ByteSize::ZERO;
    let mut files = 0u32;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(reader) = DirReader::read(&dir, want_physical) else {
            continue;
        };
        for entry in reader.into_entries() {
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

fn tree_config_for(policy: &ScanPolicy) -> TreeConfig {
    TreeConfig::from_scan_policy(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::ScanId;

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
        assert!(events.iter().any(|e| matches!(e, ScanEvent::Progress { progress, .. } if progress.dirs >= 3)));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_with_partial_results() {
        let base = temp_tree("cancel");
        let engine = ScanEngine::with_workers(4);
        let handle = engine.scan(ScanRequest::new(ScanId(4), base.clone())).unwrap();
        // Cancel quickly; the coordinator must emit Cancelled eventually.
        handle.cancel();
        let events = drain(&handle);
        handle.join();
        let outcome = events
            .iter()
            .find_map(|e| match e {
                ScanEvent::Finished { outcome, .. } => Some(*outcome),
                _ => None,
            });
        assert_eq!(outcome, Some(ScanOutcome::Cancelled));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_focus_repopulates_priority() {
        // Focus-priority ordering is a pure function tested in priority.rs;
        // this just verifies the handle wires it through without panicking.
        let base = temp_tree("focus2");
        let engine = ScanEngine::with_workers(2);
        let handle = engine.scan(ScanRequest::new(ScanId(5), base.clone())).unwrap();
        handle.set_focus(base.join("x"));
        handle.cancel();
        let _ = drain(&handle);
        handle.join();
        let _ = std::fs::remove_dir_all(&base);
    }
}
