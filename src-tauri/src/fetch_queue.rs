//! Store fill queue.
//!
//! Views never read the filesystem. A missing or stale listing/size is enqueued
//! here; workers perform the OS read, write the result back into the Store and
//! publish a revision. Listing jobs run before size jobs, and a listing that
//! discovers unseen child folders schedules them for measurement.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::analyze::AppServices;
use crate::explorer::{read_entries, recursive_size};

pub const PRIORITY_VISIBLE: u8 = 100;
pub const PRIORITY_ACTIVE: u8 = 80;
pub const PRIORITY_CHILD: u8 = 50;
pub const PRIORITY_STALE: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    List,
    Size,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Task {
    priority: u8,
    sequence: u64,
    path: String,
    kind: TaskKind,
}

impl Ord for Task {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: higher priority first, then FIFO.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for Task {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
struct QueueState {
    lists: BinaryHeap<Task>,
    active_sizes: BinaryHeap<Task>,
    background_sizes: BinaryHeap<Task>,
    queued: HashSet<(String, TaskKind)>,
    sequence: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    List,
    ActiveSize,
    BackgroundSize,
}

struct Shared {
    state: Mutex<QueueState>,
    wake: Condvar,
}

/// Deduplicating priority queue of folders that need an OS read.
#[derive(Clone)]
pub struct FetchQueue {
    shared: Arc<Shared>,
}

impl Default for FetchQueue {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(QueueState::default()),
                wake: Condvar::new(),
            }),
        }
    }
}

impl FetchQueue {
    /// Queue a folder. Returns false when the same kind is already queued.
    pub fn enqueue(&self, path: &str, kind: TaskKind, priority: u8) -> bool {
        let mut state = self.shared.state.lock().unwrap();
        if !state
            .queued
            .insert((path.to_string(), kind))
        {
            return false;
        }
        let sequence = state
            .sequence
            .fetch_add(1, AtomicOrdering::AcqRel);
        let task = Task {
            priority,
            sequence,
            path: path.to_string(),
            kind,
        };
        match kind {
            TaskKind::List => state.lists.push(task),
            TaskKind::Size if priority >= PRIORITY_ACTIVE => state.active_sizes.push(task),
            TaskKind::Size => state.background_sizes.push(task),
        }
        self.shared.wake.notify_one();
        true
    }

    pub fn enqueue_list(&self, path: &str, priority: u8) -> bool {
        self.enqueue(path, TaskKind::List, priority)
    }

    pub fn enqueue_size(&self, path: &str, priority: u8) -> bool {
        self.enqueue(path, TaskKind::Size, priority)
    }

    fn pop(&self, lane: Lane) -> Task {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            let task = match lane {
                Lane::List => state.lists.pop(),
                Lane::ActiveSize => state.active_sizes.pop(),
                Lane::BackgroundSize => state.background_sizes.pop(),
            };
            if let Some(task) = task {
                return task;
            }
            state = self.shared.wake.wait(state).unwrap();
        }
    }

    /// Release the dedupe slot after the worker finished.
    fn finish(&self, task: &Task) {
        self.shared
            .state
            .lock()
            .unwrap()
            .queued
            .remove(&(task.path.clone(), task.kind));
    }

    #[cfg(test)]
    pub(crate) fn is_queued(&self, path: &str, kind: TaskKind) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .queued
            .contains(&(path.to_string(), kind))
    }
}

/// Start the worker pool. Workers use the AppHandle to obtain the current
/// `AppServices` on every job, so they always write to the active Store.
pub fn start_workers(app: AppHandle, workers: usize) {
    let queue = app.state::<FetchQueue>().inner().clone();
    let count = workers.max(1);
    for (lane, label, workers) in [
        (Lane::List, "list", 1),
        (Lane::ActiveSize, "size-active", 1),
        (Lane::BackgroundSize, "size-bg", count),
    ] {
        for index in 0..workers {
            let app = app.clone();
            let queue = queue.clone();
            std::thread::Builder::new()
                .name(format!("sift-store-{label}-{index}"))
                .spawn(move || loop {
                    let task = queue.pop(lane);
                    match task.kind {
                        TaskKind::List => fetch_list(&app, &queue, &task.path),
                        TaskKind::Size => fetch_size(&app, &task.path),
                    }
                    queue.finish(&task);
                })
                .ok();
        }
    }
}

fn fetch_list(app: &AppHandle, queue: &FetchQueue, path: &str) {
    let services = app.state::<AppServices>();
    let store = services.store.clone();
    match read_entries(path) {
        Ok(listing) => {
            let children: Vec<String> = listing
                .entries
                .iter()
                .filter(|entry| entry.is_dir)
                .map(|entry| entry.path.clone())
                .collect();
            let entries = listing
                .entries
                .into_iter()
                .map(|entry| sift_store::FolderEntry {
                    name: entry.name,
                    path: entry.path,
                    is_dir: entry.is_dir,
                    size: entry.size,
                    modified_ms: entry.modified_ms,
                })
                .collect();
            let change = store.folders().apply(path, &listing.path, entries);
            let _ = app.emit("store://folders-changed", change);
            // The listing is cheap and already visible; child totals are the
            // expensive part. Schedule only folders the Store does not have a
            // fresh total for, at a lower priority than the visible listing.
            for child in children {
                let state = store.folders().size(&child).state;
                if state != sift_store::ListingState::Fresh {
                    let priority = if state == sift_store::ListingState::Missing {
                        PRIORITY_CHILD
                    } else {
                        PRIORITY_STALE
                    };
                    queue.enqueue_size(&child, priority);
                }
            }
        }
        Err(error) => {
            let _ = app.emit(
                "store://folders-failed",
                serde_json::json!({ "path": path, "error": error }),
            );
        }
    }
}

fn fetch_size(app: &AppHandle, path: &str) {
    let services = app.state::<AppServices>();
    let store = services.store.clone();
    let measured = recursive_size(path);
    // Keep the legacy size index coherent for features that still read it.
    if !measured.partial {
        crate::size_tree::record_scan_dir(path, measured.size);
    }
    let change = store
        .folders()
        .apply_size(path, measured.size, !measured.partial);
    let _ = app.emit("store://sizes-changed", change);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_requests_share_one_slot() {
        let queue = FetchQueue::default();
        assert!(queue.enqueue_list("/a", PRIORITY_VISIBLE));
        assert!(!queue.enqueue_list("/a", PRIORITY_VISIBLE));
        assert!(queue.is_queued("/a", TaskKind::List));
        let task = queue.pop(Lane::List);
        assert_eq!(task.path, "/a");
        assert_eq!(task.kind, TaskKind::List);
        queue.finish(&task);
        assert!(!queue.is_queued("/a", TaskKind::List));
        assert!(queue.enqueue_list("/a", PRIORITY_VISIBLE));
    }

    #[test]
    fn a_listing_and_a_size_for_one_path_are_distinct() {
        let queue = FetchQueue::default();
        assert!(queue.enqueue_list("/a", PRIORITY_VISIBLE));
        assert!(queue.enqueue_size("/a", PRIORITY_ACTIVE));
        let first = queue.pop(Lane::List);
        assert_eq!(first.kind, TaskKind::List);
        let second = queue.pop(Lane::ActiveSize);
        assert_eq!(second.kind, TaskKind::Size);
    }

    #[test]
    fn higher_priority_runs_first() {
        let queue = FetchQueue::default();
        queue.enqueue_size("/low", PRIORITY_STALE);
        queue.enqueue_list("/visible", PRIORITY_VISIBLE);
        assert_eq!(queue.pop(Lane::List).path, "/visible");
    }
}
