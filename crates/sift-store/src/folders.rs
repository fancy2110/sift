//! The Store-owned folder index.
//!
//! File lists are a read model, not a scan by-product: views ask this index for
//! a listing, and a miss returns `Missing` instead of touching the filesystem.
//! The I/O side (fetch queue) fills the index and publishes a revision; every
//! consumer can subscribe and refresh from the same snapshot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a complete listing is considered fresh.
pub const LISTING_TTL_MS: i64 = 5 * 60 * 1000;
/// Folder totals are more expensive to obtain than a listing and are reused
/// longer; a visible stale value is returned immediately while a refresh runs.
pub const SIZE_TTL_MS: i64 = 30 * 60 * 1000;

/// Bound the read model; evict the least recently fetched folder.
const MAX_LISTINGS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// File size; directories are 0 here and sized by the size index.
    pub size: u64,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListingState {
    /// Fetched within the TTL.
    Fresh,
    /// Known but older than the TTL; still useful to render while refreshing.
    Stale,
    /// Never fetched (or invalidated): the caller should enqueue a fetch.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingView {
    pub path: String,
    pub resolved: String,
    pub entries: Vec<FolderEntry>,
    pub state: ListingState,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChange {
    pub revision: u64,
    pub path: String,
    pub resolved: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderListing {
    resolved: String,
    entries: Vec<FolderEntry>,
    at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SizeRecord {
    size: u64,
    exact: bool,
    at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeView {
    pub path: String,
    pub size: Option<u64>,
    pub exact: bool,
    pub state: ListingState,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeChange {
    pub revision: u64,
    pub path: String,
    pub size: u64,
    pub exact: bool,
}

/// In-memory, Store-owned folder read model.
#[derive(Debug)]
pub struct FolderIndex {
    listings: Mutex<HashMap<String, FolderListing>>,
    sizes: Mutex<HashMap<String, SizeRecord>>,
    revision: AtomicU64,
    size_revision: AtomicU64,
    subscribers: Mutex<Vec<Sender<FolderChange>>>,
    size_subscribers: Mutex<Vec<Sender<SizeChange>>>,
}

impl Default for FolderIndex {
    fn default() -> Self {
        Self {
            listings: Mutex::new(HashMap::new()),
            sizes: Mutex::new(HashMap::new()),
            revision: AtomicU64::new(0),
            size_revision: AtomicU64::new(0),
            subscribers: Mutex::new(Vec::new()),
            size_subscribers: Mutex::new(Vec::new()),
        }
    }
}

impl FolderIndex {
    /// Read the current view. Never performs I/O.
    pub fn list(&self, requested: &str) -> ListingView {
        let revision = self.revision.load(Ordering::Acquire);
        let listing = self.listings.lock().unwrap().get(requested).cloned();
        match listing {
            None => ListingView {
                path: requested.to_string(),
                resolved: requested.to_string(),
                entries: Vec::new(),
                state: ListingState::Missing,
                revision,
            },
            Some(listing) => {
                let state = if now_ms() - listing.at_ms <= LISTING_TTL_MS {
                    ListingState::Fresh
                } else {
                    ListingState::Stale
                };
                ListingView {
                    path: requested.to_string(),
                    resolved: listing.resolved,
                    entries: listing.entries,
                    state,
                    revision,
                }
            }
        }
    }

    /// Apply a completed OS read and publish the new revision.
    pub fn apply(
        &self,
        requested: &str,
        resolved: &str,
        entries: Vec<FolderEntry>,
    ) -> FolderChange {
        {
            let mut listings = self.listings.lock().unwrap();
            if !listings.contains_key(requested) && listings.len() >= MAX_LISTINGS {
                if let Some(oldest) = listings
                    .iter()
                    .min_by_key(|(_, listing)| listing.at_ms)
                    .map(|(path, _)| path.clone())
                {
                    listings.remove(&oldest);
                }
            }
            listings.insert(
                requested.to_string(),
                FolderListing {
                    resolved: resolved.to_string(),
                    entries,
                    at_ms: now_ms(),
                },
            );
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let change = FolderChange {
            revision,
            path: requested.to_string(),
            resolved: resolved.to_string(),
        };
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|sender| sender.send(change.clone()).is_ok());
        change
    }

    /// Drop a listing so the next read is a miss. The revision is deliberately
    /// unchanged: invalidation is not a content update.
    pub fn invalidate(&self, requested: &str) {
        self.listings.lock().unwrap().remove(requested);
    }

    /// A path disappeared: drop its listing and size, and publish the removal
    /// as a zero-sized, non-exact record so views can prune immediately.
    pub fn remove_path(&self, requested: &str) {
        self.listings.lock().unwrap().remove(requested);
        self.sizes.lock().unwrap().remove(requested);
    }

    pub fn clear(&self) {
        self.listings.lock().unwrap().clear();
        self.sizes.lock().unwrap().clear();
    }

    /// Read a folder total. Never performs I/O.
    pub fn size(&self, requested: &str) -> SizeView {
        let revision = self.size_revision.load(Ordering::Acquire);
        let record = self.sizes.lock().unwrap().get(requested).cloned();
        match record {
            None => SizeView {
                path: requested.to_string(),
                size: None,
                exact: false,
                state: ListingState::Missing,
                revision,
            },
            Some(record) => SizeView {
                path: requested.to_string(),
                size: Some(record.size),
                exact: record.exact,
                state: if now_ms() - record.at_ms <= SIZE_TTL_MS {
                    ListingState::Fresh
                } else {
                    ListingState::Stale
                },
                revision,
            },
        }
    }

    /// Apply a measured folder total and publish the new revision.
    pub fn apply_size(&self, path: &str, size: u64, exact: bool) -> SizeChange {
        self.sizes.lock().unwrap().insert(
            path.to_string(),
            SizeRecord {
                size,
                exact,
                at_ms: now_ms(),
            },
        );
        let revision = self.size_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let change = SizeChange {
            revision,
            path: path.to_string(),
            size,
            exact,
        };
        let mut subscribers = self.size_subscribers.lock().unwrap();
        subscribers.retain(|sender| sender.send(change.clone()).is_ok());
        change
    }

    pub fn subscribe_sizes(&self) -> Receiver<SizeChange> {
        let (sender, receiver) = channel();
        self.size_subscribers.lock().unwrap().push(sender);
        receiver
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn subscribe(&self) -> Receiver<FolderChange> {
        let (sender, receiver) = channel();
        self.subscribers.lock().unwrap().push(sender);
        receiver
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> FolderEntry {
        FolderEntry {
            name: name.to_string(),
            path: format!("/x/{name}"),
            is_dir: true,
            size: 0,
            modified_ms: None,
        }
    }

    #[test]
    fn a_missing_listing_never_touches_disk() {
        let index = FolderIndex::default();
        let view = index.list("/x");
        assert_eq!(view.state, ListingState::Missing);
        assert!(view.entries.is_empty());
    }

    #[test]
    fn apply_publishes_a_revision_and_updates_subscribers() {
        let index = FolderIndex::default();
        let receiver = index.subscribe();
        let change = index.apply("/x", "/x", vec![entry("a")]);
        assert_eq!(change.revision, 1);
        let published = receiver.recv().unwrap();
        assert_eq!(published, change);
        let view = index.list("/x");
        assert_eq!(view.state, ListingState::Fresh);
        assert_eq!(view.entries, vec![entry("a")]);
    }

    #[test]
    fn invalidate_turns_the_next_read_into_a_miss() {
        let index = FolderIndex::default();
        index.apply("/x", "/x", vec![entry("a")]);
        index.invalidate("/x");
        assert_eq!(index.list("/x").state, ListingState::Missing);
    }

    #[test]
    fn a_size_has_its_own_revision_and_subscribers() {
        let index = FolderIndex::default();
        let receiver = index.subscribe_sizes();
        let change = index.apply_size("/x", 4096, true);
        assert_eq!(change.revision, 1);
        assert_eq!(receiver.recv().unwrap(), change);
        let view = index.size("/x");
        assert_eq!(view.size, Some(4096));
        assert_eq!(view.state, ListingState::Fresh);
    }

    #[test]
    fn removing_a_path_drops_both_listing_and_size() {
        let index = FolderIndex::default();
        index.apply("/x", "/x", vec![entry("a")]);
        index.apply_size("/x", 1, true);
        index.remove_path("/x");
        assert_eq!(index.list("/x").state, ListingState::Missing);
        assert_eq!(index.size("/x").state, ListingState::Missing);
    }
}
