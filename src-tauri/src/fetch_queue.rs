//! Store fill queue.
//!
//! The Store's folder index never reads the filesystem. When a view asks for a
//! missing or stale listing, the command enqueues it here; a small worker pool
//! performs the OS read, writes the result back into the Store and publishes a
//! `store://folders-changed` revision.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::analyze::AppServices;
use crate::explorer::read_entries;

#[derive(Default)]
struct QueueState {
    pending: VecDeque<String>,
    queued: HashSet<String>,
}

struct Shared {
    state: Mutex<QueueState>,
    wake: Condvar,
}

/// Deduplicating FIFO of folders that need an OS read.
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
    /// Queue one folder. Returns false when it is already queued/in flight.
    pub fn enqueue(&self, path: &str) -> bool {
        let mut state = self.shared.state.lock().unwrap();
        if !state.queued.insert(path.to_string()) {
            return false;
        }
        state.pending.push_back(path.to_string());
        self.shared.wake.notify_one();
        true
    }

    fn pop(&self) -> String {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(path) = state.pending.pop_front() {
                return path;
            }
            state = self.shared.wake.wait(state).unwrap();
        }
    }

    /// Release the dedupe slot after the worker finished.
    fn finish(&self, path: &str) {
        self.shared.state.lock().unwrap().queued.remove(path);
    }

    #[cfg(test)]
    pub(crate) fn is_queued(&self, path: &str) -> bool {
        self.shared.state.lock().unwrap().queued.contains(path)
    }
}

/// Start the worker pool. Workers use the AppHandle to obtain the current
/// `AppServices` on every job, so they always write to the active Store.
pub fn start_workers(app: AppHandle, workers: usize) {
    let queue = app.state::<FetchQueue>().inner().clone();
    for index in 0..workers.max(1) {
        let app = app.clone();
        let queue = queue.clone();
        std::thread::Builder::new()
            .name(format!("sift-folder-fetch-{index}"))
            .spawn(move || loop {
                let path = queue.pop();
                fetch_one(&app, &path);
                queue.finish(&path);
            })
            .ok();
    }
}

fn fetch_one(app: &AppHandle, path: &str) {
    let services = app.state::<AppServices>();
    let store = services.store.clone();
    match read_entries(path) {
        Ok(listing) => {
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
        }
        Err(error) => {
            let _ = app.emit(
                "store://folders-failed",
                serde_json::json!({ "path": path, "error": error }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_requests_share_one_slot() {
        let queue = FetchQueue::default();
        assert!(queue.enqueue("/a"));
        assert!(!queue.enqueue("/a"));
        assert!(queue.is_queued("/a"));
        assert_eq!(queue.pop(), "/a");
        queue.finish("/a");
        assert!(!queue.is_queued("/a"));
        assert!(queue.enqueue("/a"));
    }
}
