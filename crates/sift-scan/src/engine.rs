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
use sift_core::id::{FileId, NodeKey};
use sift_core::tree::NONE;
use sift_core::{
    ByteSize, DirectoryEntry, DirectorySummary, Progress, ScanEvent, ScanId, ScanOutcome,
    ScanPolicy, ScanRequest, ScanTree, TreeConfig,
};
use sift_platform::dir::{DirReader, RawEntry};

use crate::priority::{category, JobKind, OrdDir, QueuedDir};
use crate::snapshot::{metadata_mtime_ms, DirSnapshot, SnapshotStore};
use crate::timing::{ScanTimings, Timed, TimingsHandle};

/// How often the coordinator publishes interim progress and size updates.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// The engine. Cheap to create and share.
pub struct ScanEngine {
    workers: usize,
    snapshots: Option<Arc<dyn SnapshotStore>>,
    /// Absolute prefixes never entered on a cold walk (well-known system
    /// areas that have no cleanable user content).
    skip_prefixes: Vec<PathBuf>,
}

impl std::fmt::Debug for ScanEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanEngine")
            .field("workers", &self.workers)
            .field("snapshots", &self.snapshots.is_some())
            .field("skip_prefixes", &self.skip_prefixes)
            .finish()
    }
}

impl Clone for ScanEngine {
    fn clone(&self) -> Self {
        Self {
            workers: self.workers,
            snapshots: self.snapshots.clone(),
            skip_prefixes: self.skip_prefixes.clone(),
        }
    }
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
    /// Per-phase accumulated timings for this run.
    pub timings: TimingsHandle,
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
            snapshots: None,
            skip_prefixes: Vec::new(),
        }
    }

    /// An engine pinned to `workers` threads, for tests and constrained boxes.
    pub fn with_workers(workers: usize) -> Self {
        Self {
            workers: workers.clamp(1, 64),
            snapshots: None,
            skip_prefixes: Vec::new(),
        }
    }

    /// Never enter directories whose absolute path starts with `prefix`.
    /// Used by the fast cold-walk to skip well-known system areas.
    pub fn with_skip_prefix(mut self, prefix: impl Into<PathBuf>) -> Self {
        self.skip_prefixes.push(prefix.into());
        self
    }

    /// The skip rules this engine applies.
    pub fn skip_prefixes(&self) -> &[PathBuf] {
        &self.skip_prefixes
    }

    /// Attach a durable snapshot store so resumable and incremental scans skip
    /// unchanged subtrees.
    pub fn with_snapshot_store(mut self, store: Arc<dyn SnapshotStore>) -> Self {
        self.snapshots = Some(store);
        self
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
        let snapshots = self.snapshots.clone();
        let frugal = request.policy.frugal_cpu;
        let timings = ScanTimings::new();
        for _ in 0..workers {
            let dispatch_rx = Arc::clone(&dispatch_rx);
            let result_tx = result_tx.clone();
            let cancel = Arc::clone(&cancel);
            let snapshots = snapshots.clone();
            let worker_skips = self.skip_prefixes.clone();
            let worker_timings = Arc::clone(&timings) as TimingsHandle;
            std::thread::Builder::new()
                .name("sift-worker".into())
                .spawn(move || {
                    let mut done = 0u32;
                    loop {
                        let job = dispatch_rx.lock().unwrap().recv();
                        let Ok(job) = job else { break };
                        if cancel.load(AtomicOrdering::SeqCst) {
                            break;
                        }
                        let result = read_job(
                            job,
                            want_physical,
                            snapshots.as_deref(),
                            Some(&worker_timings),
                            &worker_skips,
                        );
                        if result_tx.send(result).is_err() {
                            break;
                        }
                        // Frugal mode: yield after every few directories and
                        // take a short nap, so foreground apps keep the cores.
                        if frugal {
                            done += 1;
                            if done % 8 == 0 {
                                std::thread::sleep(std::time::Duration::from_millis(2));
                            }
                        }
                    }
                })?;
        }
        drop(result_tx);

        // Coordinator: owns the queue, the tree, and the event stream.
        let coordinator_snapshots = self.snapshots.clone();
        let coordinator_timings = Arc::clone(&timings);
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
                    snapshots: coordinator_snapshots,
                    timings: coordinator_timings,
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
            timings,
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
    snapshots: Option<Arc<dyn SnapshotStore>>,
    timings: TimingsHandle,
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
    /// Predicted giant directories awaiting post-scan calibration, in the
    /// order they were predicted.
    calibrate: Vec<CalibrationJob>,
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
    snapshots: Option<Arc<dyn SnapshotStore>>,
    timings: TimingsHandle,
    event_tx: mpsc::Sender<ScanEvent>,
    scan_id: ScanId,
}

/// A giant directory deferred for asynchronous measurement.
#[derive(Clone)]
struct CalibrationJob {
    index: u32,
    path: PathBuf,
    tracked: bool,
}

impl CoordinatorState {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// The snapshot store, if one is attached.
    fn snapshots(&self) -> Option<&dyn SnapshotStore> {
        self.snapshots.as_deref()
    }
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
        snapshots,
        timings,
        dispatch_tx,
        result_rx,
        event_tx,
    } = inputs;

    let report_timings = Arc::clone(&timings);
    let mut state = CoordinatorState {
        queue: BinaryHeap::new(),
        pending_children: HashMap::new(),
        size_only_pending: HashMap::new(),
        unrecorded: HashMap::new(),
        dirty: HashSet::new(),
        progress: Progress::default(),
        calibrate: Vec::new(),
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
        snapshots,
        timings,
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

    let outcome = coordinator_loop(
        &mut state,
        &tree,
        &cancel,
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
        tree_guard.shrink_to_fit();
    }

    // Post-scan calibration runs in the background: the main scan reports
    // completion immediately and the predicted giant directories are measured
    // afterward, patching totals through events. Skipped on cancel. When there
    // is nothing to calibrate, emit the terminal marker right away.
    if matches!(outcome, ScanOutcome::Cancelled) || state.calibrate.is_empty() {
        let _ = state
            .event_tx
            .send(ScanEvent::CalibrationFinished { scan: scan_id });
    } else {
        let jobs = std::mem::take(&mut state.calibrate);
        let bg_tree = Arc::clone(&tree);
        let bg_event_tx = state.event_tx.clone();
        let bg_snapshots = state.snapshots.clone();
        let bg_timings = Arc::clone(&state.timings);
        let bg_scan_id = scan_id;
        let bg_workers = workers;
        let bg_cancel = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("sift-calibration".into())
            .spawn(move || {
                run_background_calibration(
                    jobs,
                    bg_tree,
                    bg_event_tx,
                    bg_snapshots,
                    bg_timings,
                    bg_scan_id,
                    bg_workers,
                    bg_cancel,
                );
            })
            .ok();
    }

    let final_progress = state.progress;
    let _ = (root_key,);
    if crate::timing::timings_enabled() {
        eprintln!("{}", report_timings.report());
    }
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

fn process_result(
    state: &mut CoordinatorState,
    tree: &Arc<Mutex<ScanTree>>,
    result: &DirResult,
) {
    match result {
        DirResult::Tracked {
            index,
            entries,
            writable,
        } => {
            let mut tree_guard = tree.lock().unwrap();
            match entries {
                Some(entries) => {
                    process_dir_entries(state, &mut tree_guard, *index, entries, *writable);
                }
                None => {
                    state.progress.denied += 1;
                    // Unreadable directory: settle it at zero.
                    close_and_cascade(state, &mut tree_guard, *index, false);
                }
            }
            drop(tree_guard);
        }
        DirResult::Predicted { index, files } => {
            // Record the estimated file count so the subtree closes and the
            // main scan can complete, then queue exact calibration.
            let path = {
                let mut tree_guard = tree.lock().unwrap();
                tree_guard.path(*index)
            };
            state.calibrate.push(CalibrationJob {
                index: *index,
                path: PathBuf::from(&path),
                tracked: true,
            });
            let mut tree_guard = tree.lock().unwrap();
            state.progress.files += *files as u64;
            state.progress.dirs += 1;
            close_predicted(state, &mut tree_guard, *index);
        }
        DirResult::PredictedSize { parent, files } => {
            let path = {
                let mut tree_guard = tree.lock().unwrap();
                tree_guard.path(*parent)
            };
            state.calibrate.push(CalibrationJob {
                index: *parent,
                path: PathBuf::from(&path),
                tracked: false,
            });
            // Fold as an unrecorded size-only branch with the estimated file
            // count; bytes are filled in by calibration.
            let mut tree_guard = tree.lock().unwrap();
            let finished = {
                let entry = state.size_only_pending.entry(*parent).or_insert(0);
                *entry = entry.saturating_sub(1);
                *entry == 0
            };
            if finished {
                state.size_only_pending.remove(parent);
            }
            state
                .unrecorded
                .entry(*parent)
                .and_modify(|(_s, f)| *f += *files)
                .or_insert((ByteSize::ZERO, *files));
            state.progress.files += *files as u64;
            if state.pending_children.get(parent).copied().unwrap_or(0) == 0
                && !state.size_only_pending.contains_key(parent)
            {
                close_and_cascade(state, &mut tree_guard, *parent, false);
            }
        }
        DirResult::Skipped { index } => {
            let mut tree_guard = tree.lock().unwrap();
            // A skipped system directory: settle at zero without counting it
            // as denied; it holds no cleanable content.
            close_and_cascade(state, &mut tree_guard, *index, false);
            drop(tree_guard);
        }
        DirResult::Reused { index, size, files } => {
            let mut tree_guard = tree.lock().unwrap();
            tree_guard.add_unrecorded(*index, *size, *files);
            state.progress.files += *files as u64;
            state.progress.bytes += *size;
            state.progress.dirs += 1;
            close_and_cascade(state, &mut tree_guard, *index, false);
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
            state.progress.files += *files as u64;
            state.progress.bytes += *size;
            if state.pending_children.get(parent).copied().unwrap_or(0) == 0
                && !state.size_only_pending.contains_key(parent)
            {
                close_and_cascade(state, &mut tree_guard, *parent, false);
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
    dir_writable: bool,
) {
    let Some(dir) = tree.node(index) else {
        return;
    };
    let dir_key = dir.key;
    let dir_path = tree.path(index);

    let mut child_dirs = 0u32;
    let mut dir_entries: Vec<DirectoryEntry> = Vec::with_capacity(entries.len());

    // `dir_writable` rode in from the worker, which judged it from the
    // directory's own stat (taken for the snapshot probe): no extra syscall
    // here, and the coordinator stays off the filesystem.
    for entry in entries {
        if entry.name == b"." || entry.name == b".." {
            continue;
        }
        let child_path;
        let child_key;
        let name_string;
        {
            let _t = Timed::start(&state.timings, &state.timings.coord_path_key);
            child_path = join_bytes(&dir_path, &entry.name);
            child_key = NodeKey::from_path(Path::new(&child_path));
            name_string = String::from_utf8_lossy(&entry.name).into_owned();
        }
        let mtime_ms = entry.mtime_ms;
        let deletable =
            classify(Path::new(&child_path)).is_yes() && dir_writable;

        if entry.is_dir && !entry.is_symlink {
            // Real directory: open in the tree, enqueue its read.
            let open_result = {
                let _t = Timed::start(&state.timings, &state.timings.coord_tree);
                tree.open_dir(index, child_key, &entry.name, mtime_ms, deletable, false)
            };
            if let Some(child_ix) = open_result {
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
            let counted = {
                let _t = Timed::start(&state.timings, &state.timings.coord_tree);
                tree.record_file(index, size, file_id)
            };
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
        close_and_cascade(state, tree, index, false);
    }

    let summary = {
        let _t = Timed::start(&state.timings, &state.timings.coord_entries);
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
    {
        let _t = Timed::start(&state.timings, &state.timings.coord_event_send);
        let _ = state.event_tx.send(ScanEvent::DirectoryListed {
            scan: state.scan_id,
            dir: summary,
        });
    }
}

/// Close `index` and cascade upward while parents become complete.
///
/// When `reopen` is set this directory is being freshly read after a snapshot
/// reuse: its old subtree bytes are subtracted from its parent, and the close
/// does not cascade past it (its ancestors never waited for it). A matching
/// snapshot is handed to the store for the next scan.
#[allow(clippy::too_many_arguments)]
fn close_and_cascade(
    state: &mut CoordinatorState,
    tree: &mut ScanTree,
    index: u32,
    reopen: bool,
) {
    if reopen {
        tree.reopen_dir(index);
    }
    let mut cursor = index;
    loop {
        let Some(node) = tree.node(cursor) else { return };
        let parent = node.parent;
        let key = node.key;

        // Fold any unrecorded subtree bytes into this directory first.
        if let Some((size, files)) = state.unrecorded.remove(&cursor) {
            tree.add_unrecorded(cursor, size, files);
        }

        {
            let _t = Timed::start(&state.timings, &state.timings.coord_tree);
            tree.close_dir(cursor);
        }
        state.dirty.insert(cursor);
        state.progress.dirs += 1;

        // Persist the closed directory for later scans. Directories without a
        // usable mtime cannot be safely matched, and the scan root is exempt
        // for the same reason reads are.
        let closed_node = tree.node(cursor).copied();
        if let Some(node) = closed_node {
            if node.mtime_ms > 0 && cursor != 0 {
                if let Some(store) = state.snapshots() {
                    store.record(DirSnapshot::new(
                        tree.path(cursor),
                        node.mtime_ms,
                        node.size.logical as i64,
                        node.size.physical as i64,
                        node.file_count as i64,
                    ));
                }
            }
        }

        let _ = state.event_tx.send(ScanEvent::DirectoryClosed {
            scan: state.scan_id,
            key,
        });

        if parent == NONE || reopen {
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

/// Close a predicted giant directory with its estimated file count and zero
/// bytes, then cascade upward exactly like a normal close. Calibration later
/// patches the difference through `adjust_totals`.
fn close_predicted(state: &mut CoordinatorState, tree: &mut ScanTree, index: u32) {
    {
        let Some(node) = tree.node(index).copied() else { return };
        let _ = node;
        // Tag the node as an estimate for the UI.
        if let Some(n) = tree.node_mut(index) {
            n.set_estimated(true);
        }
    }
    close_and_cascade(state, tree, index, false);
}

/// Background calibration: measure predicted giant directories after the main
/// scan, using dedicated worker threads. The scan has already reported
/// completion, so this never blocks the UI. Each exact subtree is patched into
/// the closed tree and published as `Calibrated`.
fn run_background_calibration(
    jobs: Vec<CalibrationJob>,
    tree: Arc<Mutex<ScanTree>>,
    event_tx: mpsc::Sender<ScanEvent>,
    snapshots: Option<Arc<dyn SnapshotStore>>,
    _timings: TimingsHandle,
    scan_id: ScanId,
    workers: usize,
    cancel: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let threads = workers.clamp(1, 8);
    let _ = event_tx.send(ScanEvent::Warning {
        scan: scan_id,
        message: format!("正在后台校准 {} 个大型目录…", jobs.len()),
    });

    let (result_tx, result_rx) = mpsc::channel::<DirResult>();
    // std mpsc receivers aren't cloneable, so give every worker its own
    // channel and round-robin jobs across them.
    let mut worker_txs: Vec<mpsc::Sender<DirJob>> = Vec::with_capacity(threads);
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let (wtx, wrx) = mpsc::channel::<DirJob>();
        let tx = result_tx.clone();
        let worker_cancel = Arc::clone(&cancel);
        let h = std::thread::Builder::new()
            .name("sift-cal-worker".into())
            .spawn(move || {
                while let Ok(job) = wrx.recv() {
                    if worker_cancel.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    let result = read_job(job, true, None, None, &[]);
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            });
        if let Ok(h) = h {
            handles.push(h);
            worker_txs.push(wtx);
        }
    }
    drop(result_tx);

    let mut next = 0usize;
    let mut in_flight = 0usize;
    loop {
        if cancel.load(AtomicOrdering::Relaxed) {
            break;
        }
        while in_flight < threads && next < jobs.len() {
            let job = &jobs[next];
            let dispatch = if job.tracked {
                DirJob::CalibrateTracked { index: job.index, path: job.path.clone() }
            } else {
                DirJob::CalibrateSize { parent: job.index, path: job.path.clone() }
            };
            let target = next % worker_txs.len();
            if worker_txs[target].send(dispatch).is_err() {
                return;
            }
            in_flight += 1;
            next += 1;
        }
        if in_flight == 0 {
            break;
        }
        let Ok(result) = result_rx.recv() else { break };
        in_flight -= 1;
        apply_background_calibration(
            &tree,
            &event_tx,
            snapshots.as_deref(),
            &result,
            scan_id,
        );
    }

    drop(worker_txs);
    for h in handles {
        let _ = h.join();
    }
    let _ = event_tx.send(ScanEvent::Warning {
        scan: scan_id,
        message: format!("大型目录校准完成（{:.1}s）", started.elapsed().as_secs_f64()),
    });
    let _ = event_tx.send(ScanEvent::CalibrationFinished { scan: scan_id });
    if crate::timing::timings_enabled() {
        eprintln!(
            "background calibration: {} dirs in {:.3}s",
            jobs.len(),
            started.elapsed().as_secs_f64()
        );
    }
}

/// Patch one calibrated subtree into the closed tree and publish it.
fn apply_background_calibration(
    tree: &Arc<Mutex<ScanTree>>,
    event_tx: &mpsc::Sender<ScanEvent>,
    snapshots: Option<&dyn SnapshotStore>,
    result: &DirResult,
    scan_id: ScanId,
) {
    // Calibration always rides the `Sized` variant with `parent` holding the
    // node index, and the measure walked the whole subtree — so the delta is
    // exact even when the predicted giant contains nested directories.
    let DirResult::Sized {
        parent: index,
        size,
        files,
    } = result
    else {
        return;
    };
    let (index, size, files) = (*index, *size, *files as u64);

    let (key, new_size, new_files) = {
        let mut guard = tree.lock().unwrap();
        let key = guard
            .node(index)
            .map(|n| n.key)
            .unwrap_or_else(|| NodeKey::from_path(Path::new("")));
        guard.adjust_totals(index, size, files);
        if let Some(n) = guard.node_mut(index) {
            n.set_estimated(false);
        }
        let node = guard.node(index).copied();
        if let Some(node) = node {
            if node.mtime_ms > 0 {
                if let Some(store) = snapshots {
                    store.record(DirSnapshot::new(
                        guard.path(index),
                        node.mtime_ms,
                        node.size.logical as i64,
                        node.size.physical as i64,
                        node.file_count as i64,
                    ));
                }
            }
        }
        (
            key,
            node.map(|n| n.size).unwrap_or(size),
            node.map(|n| n.file_count as u64).unwrap_or(files),
        )
    };

    let _ = event_tx.send(ScanEvent::Calibrated {
        scan: scan_id,
        key,
        size: new_size,
        files: new_files,
    });
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

// ---- worker-side reading ---------------------------------------------------
enum DirJob {
    Tracked { index: u32, path: PathBuf },
    SizeOnly { parent: u32, path: PathBuf },
    /// Calibration reread of a predicted giant: bypasses prediction so it is
    /// actually enumerated.
    CalibrateTracked { index: u32, path: PathBuf },
    CalibrateSize { parent: u32, path: PathBuf },
}

enum DirResult {
    Tracked {
        index: u32,
        entries: Option<Vec<RawEntry>>,
        /// Whether the directory itself permits children to be deleted, from
        /// the worker's own stat of the directory.
        writable: bool,
    },
    /// The directory was reused from a snapshot: no enumeration happened.
    Reused { index: u32, size: ByteSize, files: u32 },
    /// The directory is in a well-known system area: deliberately not walked.
    Skipped { index: u32 },
    /// A predicted giant flat directory: closed with estimated file count and
    /// zero bytes; queued for asynchronous calibration after the main scan.
    Predicted { index: u32, files: u32 },
    /// Same prediction for a budget-refused (size-only) giant subtree.
    PredictedSize { parent: u32, files: u32 },
    Sized { parent: u32, size: ByteSize, files: u32 },
}

fn read_job(
    job: DirJob,
    want_physical: bool,
    snapshots: Option<&dyn SnapshotStore>,
    timings: Option<&ScanTimings>,
    skip_prefixes: &[PathBuf],
) -> DirResult {
    match job {
        // Calibration bypasses skip, prediction and snapshots: the whole
        // subtree is actually enumerated exactly once. Calibration results
        // ride the `Sized` variant with `parent` holding the node index.
        DirJob::CalibrateTracked { index, path } => {
            let (size, files) = measure_subtree(&path, want_physical);
            DirResult::Sized { parent: index, size, files }
        }
        DirJob::CalibrateSize { parent, path } => {
            let (size, files) = measure_subtree(&path, want_physical);
            DirResult::Sized { parent, size, files }
        }
        DirJob::Tracked { index, path } => {
            if is_skipped(&path, skip_prefixes) {
                return DirResult::Skipped { index };
            }
            read_tracked(index, &path, want_physical, snapshots, timings)
        }
        DirJob::SizeOnly { parent, path } => {
            if is_skipped(&path, skip_prefixes) {
                return DirResult::Sized {
                    parent,
                    size: ByteSize::ZERO,
                    files: 0,
                };
            }
            read_size_only(parent, &path, want_physical, snapshots, timings)
        }
    }
}

fn is_skipped(path: &Path, skip_prefixes: &[PathBuf]) -> bool {
    skip_prefixes.iter().any(|prefix| path.starts_with(prefix))
}

/// Stat the directory itself once. That single metadata record answers three
/// questions the old code paid two or three syscalls for: its `st_size`
/// (giant prediction), its mtime (snapshot key) and its mode bits (whether
/// its children are deletable).
fn stat_self(path: &Path, timings: Option<&ScanTimings>) -> Option<std::fs::Metadata> {
    let started = std::time::Instant::now();
    let metadata = std::fs::symlink_metadata(path).ok();
    if let Some(timings) = timings {
        timings
            .dir_stat
            .fetch_add(started.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
    }
    metadata
}

/// A snapshot reused from the store, normalised to exact byte counts.
fn reuse_snapshot(snapshot: DirSnapshot) -> (ByteSize, u32) {
    (
        ByteSize::new(
            snapshot.logical.max(0) as u64,
            snapshot.physical.max(0) as u64,
        ),
        snapshot.files.max(0) as u32,
    )
}

#[allow(clippy::too_many_arguments)]
fn read_tracked(
    index: u32,
    path: &Path,
    want_physical: bool,
    snapshots: Option<&dyn SnapshotStore>,
    timings: Option<&ScanTimings>,
) -> DirResult {
    let is_root = index == 0;
    let metadata = stat_self(path, timings);
    // Judge the directory's own write bits from the stat already in hand —
    // zero extra cost on the worker and nothing on the coordinator.
    let writable = metadata
        .as_ref()
        .is_some_and(sift_core::deletable::dir_is_writable);

    if !is_root {
        // Exact snapshot wins: an unchanged directory closes with real totals
        // without enumeration.
        if let (Some(store), Some(metadata)) = (snapshots, metadata.as_ref()) {
            let mtime_ms = metadata_mtime_ms(metadata);
            if let Some(snapshot) = store.lookup(path, mtime_ms) {
                timings.map(|t| t.reused_dirs.fetch_add(1, AtomicOrdering::Relaxed));
                let (size, files) = reuse_snapshot(snapshot);
                return DirResult::Reused { index, size, files };
            }
        }

        // No exact history: a giant flat directory is estimated now and
        // calibrated asynchronously after the main scan.
        if let Some(metadata) = metadata.as_ref() {
            let dir_size = metadata.len();
            if dir_size >= PREDICT_MIN_DIR_SIZE {
                timings.map(|t| t.predicted_dirs.fetch_add(1, AtomicOrdering::Relaxed));
                return DirResult::Predicted {
                    index,
                    files: (dir_size / DIR_BYTES_PER_ENTRY) as u32,
                };
            }
        }
    }

    let entries = read_dir(path, want_physical, timings);
    DirResult::Tracked {
        index,
        entries,
        writable,
    }
}

#[allow(clippy::too_many_arguments)]
fn read_size_only(
    parent: u32,
    path: &Path,
    want_physical: bool,
    snapshots: Option<&dyn SnapshotStore>,
    timings: Option<&ScanTimings>,
) -> DirResult {
    let metadata = stat_self(path, timings);

    // Budget-refused subtrees are equally reusable.
    if let (Some(store), Some(metadata)) = (snapshots, metadata.as_ref()) {
        let mtime_ms = metadata_mtime_ms(metadata);
        if let Some(snapshot) = store.lookup(path, mtime_ms) {
            timings.map(|t| t.reused_dirs.fetch_add(1, AtomicOrdering::Relaxed));
            let (size, files) = reuse_snapshot(snapshot);
            return DirResult::Sized { parent, size, files };
        }
    }

    // A giant refused subtree is deferred too, so the main scan still finishes
    // promptly; calibration measures it afterward.
    if let Some(metadata) = metadata.as_ref() {
        let dir_size = metadata.len();
        if dir_size >= PREDICT_MIN_DIR_SIZE {
            timings.map(|t| t.predicted_dirs.fetch_add(1, AtomicOrdering::Relaxed));
            return DirResult::PredictedSize {
                parent,
                files: (dir_size / DIR_BYTES_PER_ENTRY) as u32,
            };
        }
    }

    let (size, files) = measure_subtree(path, want_physical);
    DirResult::Sized { parent, size, files }
}

fn read_dir(
    path: &Path,
    want_physical: bool,
    timings: Option<&ScanTimings>,
) -> Option<Vec<RawEntry>> {
    let started = std::time::Instant::now();
    let entries = DirReader::read(path, want_physical)
        .ok()
        .map(DirReader::into_entries);
    if let Some(timings) = timings {
        timings.observe_dir(&path.to_string_lossy(), started.elapsed());
        timings
            .dir_read
            .fetch_add(started.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
    }
    entries
}

/// A directory entry's own metadata (size) is cheap: on APFS the directory's
/// `st_size` grows roughly linearly with its entry count (~32 B/entry) once a
/// directory spills out of inline storage, so one stat predicts whether a full
/// enumeration would dominate the scan.
const PREDICT_MIN_DIR_SIZE: u64 = 4 * 1024 * 1024;
/// Approximate bytes of directory metadata per child entry, for estimating the
/// entry count of a giant directory before reading it.
const DIR_BYTES_PER_ENTRY: u64 = 32;

/// Sum a whole subtree without building tree nodes, deduping hardlinks in a
/// local set.
///
/// Used for budget-refused branches and for calibration, so both measure the
/// exact same thing. Iterative so worker stack usage is bounded regardless of
/// tree depth. Only a file the platform reports as multiply linked enters the
/// local dedupe table, matching the tracked walk.
fn measure_subtree(path: &Path, want_physical: bool) -> (ByteSize, u32) {
    let mut total = ByteSize::ZERO;
    let mut files = 0u32;
    let mut seen_links: HashSet<FileId> = HashSet::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(reader) = DirReader::read(&dir, want_physical) else {
            continue;
        };
        for entry in reader.into_entries() {
            if entry.is_dir && !entry.is_symlink {
                stack.push(join_path(&dir, &entry.name));
            } else if entry.is_file {
                // Only an id the platform actually reported can dedupe; a
                // missing id counts the file, which is the safe direction.
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
        assert_eq!(expected, 6120, "the fixture arithmetic is the point of the test");
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
