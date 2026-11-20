//! The Store-owned folder index.
//!
//! File lists are a read model, not a scan by-product: views ask this index for
//! a listing, and a miss returns `Missing` instead of touching the filesystem.
//! The I/O side (fetch queue) fills the index and publishes a revision; every
//! consumer can subscribe and refresh from the same snapshot.

use std::collections::{HashMap, HashSet};
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

/// One directory announced by the scan engine. Scan ingestion feeds these
/// into the same tree as listing-derived nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanNode {
    pub path: String,
    pub parent: Option<String>,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub exact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub path: String,
    pub name: String,
    pub parent: Option<String>,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub exact: bool,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeRecord {
    parent: Option<String>,
    is_dir: bool,
    purpose: Option<String>,
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
    nodes: Mutex<HashMap<String, NodeRecord>>,
    children: Mutex<HashMap<String, HashSet<String>>>,
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
            nodes: Mutex::new(HashMap::new()),
            children: Mutex::new(HashMap::new()),
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
                    entries: entries.clone(),
                    at_ms: now_ms(),
                },
            );
        }
        self.register_listing(requested, &entries);
        // A parent's total is the sum of its direct children (including
        // direct files). Recompute it as soon as the listing supplies edges.
        let exact_parent = self
            .sizes
            .lock()
            .unwrap()
            .get(requested)
            .map(|record| record.exact)
            .unwrap_or(false);
        if !exact_parent {
            let children = self
                .children
                .lock()
                .unwrap()
                .get(requested)
                .cloned()
                .unwrap_or_default();
            let sizes = self.sizes.lock().unwrap();
            let known = children
                .iter()
                .filter_map(|child| sizes.get(child).map(|record| record.size))
                .sum::<u64>();
            let has_known = children.iter().any(|child| sizes.contains_key(child));
            drop(sizes);
            if has_known {
                self.apply_size(requested, known, false);
            }
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

    fn register_listing(&self, requested: &str, entries: &[FolderEntry]) {
        let parent = std::path::Path::new(requested)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().into_owned());
        let mut nodes = self.nodes.lock().unwrap();
        nodes.entry(requested.to_string()).or_insert(NodeRecord {
            parent,
            is_dir: true,
            purpose: None,
        });
        for entry in entries {
            nodes
                .entry(entry.path.clone())
                .and_modify(|node| node.is_dir = entry.is_dir)
                .or_insert(NodeRecord {
                    parent: Some(requested.to_string()),
                    is_dir: entry.is_dir,
                    purpose: None,
                });
        }
        drop(nodes);
        let mut children = self.children.lock().unwrap();
        children.insert(
            requested.to_string(),
            entries.iter().map(|entry| entry.path.clone()).collect(),
        );
        drop(children);
        let mut sizes = self.sizes.lock().unwrap();
        for entry in entries {
            if !entry.is_dir {
                sizes.insert(
                    entry.path.clone(),
                    SizeRecord {
                        size: entry.size,
                        exact: true,
                        at_ms: now_ms(),
                    },
                );
            }
        }
    }

    /// Apply directories discovered/sized by the scan engine. This is the
    /// write path that makes the Store, not the scan tree, the shared result.
    pub fn apply_scan_nodes(&self, nodes: &[ScanNode]) {
        if nodes.is_empty() {
            return;
        }
        {
            let mut tree = self.nodes.lock().unwrap();
            let mut children = self.children.lock().unwrap();
            let mut sizes = self.sizes.lock().unwrap();
            for node in nodes {
                tree.entry(node.path.clone())
                    .and_modify(|record| record.is_dir = node.is_dir)
                    .or_insert(NodeRecord {
                        parent: node.parent.clone(),
                        is_dir: node.is_dir,
                        purpose: None,
                    });
                if let Some(parent) = &node.parent {
                    children
                        .entry(parent.clone())
                        .or_default()
                        .insert(node.path.clone());
                }
                if let Some(size) = node.size {
                    sizes.insert(
                        node.path.clone(),
                        SizeRecord {
                            size,
                            exact: node.exact,
                            at_ms: now_ms(),
                        },
                    );
                }
            }
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.size_revision.fetch_add(1, Ordering::AcqRel);
    }

    /// One node from the Store tree, including its latest known size.
    pub fn node(&self, path: &str) -> Option<NodeView> {
        let nodes = self.nodes.lock().unwrap();
        let node = nodes.get(path)?;
        let size = self.sizes.lock().unwrap().get(path).cloned();
        Some(NodeView {
            path: path.to_string(),
            name: std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string()),
            parent: node.parent.clone(),
            is_dir: node.is_dir,
            size: size.as_ref().map(|record| record.size),
            exact: size.map(|record| record.exact).unwrap_or(false),
            purpose: node.purpose.clone(),
        })
    }

    /// Store the user/analysis-provided purpose for a node.
    pub fn set_purpose(&self, path: &str, purpose: Option<String>) -> bool {
        let mut nodes = self.nodes.lock().unwrap();
        match nodes.get_mut(path) {
            Some(node) => {
                node.purpose = purpose;
                true
            }
            None => false,
        }
    }

    /// Direct children recorded for a folder, sorted by name.
    pub fn children(&self, path: &str) -> Vec<NodeView> {
        let children = self
            .children
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .unwrap_or_default();
        let mut out: Vec<NodeView> = children
            .iter()
            .filter_map(|child| self.node(child))
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
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
        let mut stack = vec![requested.to_string()];
        let mut removed = HashSet::new();
        while let Some(path) = stack.pop() {
            if !removed.insert(path.clone()) {
                continue;
            }
            if let Some(children) = self.children.lock().unwrap().remove(&path) {
                stack.extend(children);
            }
            self.nodes.lock().unwrap().remove(&path);
            self.listings.lock().unwrap().remove(&path);
            self.sizes.lock().unwrap().remove(&path);
        }
        if let Some(parent) = std::path::Path::new(requested)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
        {
            self.children
                .lock()
                .unwrap()
                .entry(parent)
                .or_default()
                .remove(requested);
        }
    }

    pub fn clear(&self) {
        self.listings.lock().unwrap().clear();
        self.sizes.lock().unwrap().clear();
        self.nodes.lock().unwrap().clear();
        self.children.lock().unwrap().clear();
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

    /// Fast path for the scan pump: record the directory's own size without
    /// walking ancestors. A full scan reports every directory, so parent totals
    /// arrive from the engine; this keeps the hot path O(1) per directory.
    pub fn apply_measured_size(&self, path: &str, size: u64, exact: bool) -> SizeChange {
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

    /// Apply a measured folder total and publish the new revision. Ancestors
    /// are recomputed from their direct children so `/Users` reflects
    /// `/Users/xiaocy` even when only the child was measured.
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
        let mut changes = vec![change.clone()];
        changes.extend(self.recompute_size_ancestors(path, revision));
        let mut subscribers = self.size_subscribers.lock().unwrap();
        for change in changes {
            subscribers.retain(|sender| sender.send(change.clone()).is_ok());
        }
        change
    }

    /// Walk upward from a changed child. An exact ancestor already includes
    /// its children and is left untouched; a missing/derived ancestor becomes
    /// the sum of all currently known direct children.
    fn recompute_size_ancestors(&self, path: &str, revision: u64) -> Vec<SizeChange> {
        let children = self.children.lock().unwrap().clone();
        let mut sizes = self.sizes.lock().unwrap();
        let mut out = Vec::new();
        let mut current = path.to_string();
        while let Some(parent) = std::path::Path::new(&current)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
        {
            if parent.is_empty() || parent == current {
                break;
            }
            if sizes
                .get(&parent)
                .map(|record| record.exact)
                .unwrap_or(false)
            {
                break;
            }
            let Some(kids) = children.get(&parent) else {
                break;
            };
            let total: u64 = kids
                .iter()
                .filter_map(|child| sizes.get(child).map(|record| record.size))
                .sum();
            sizes.insert(
                parent.clone(),
                SizeRecord {
                    size: total,
                    exact: false,
                    at_ms: now_ms(),
                },
            );
            out.push(SizeChange {
                revision,
                path: parent.clone(),
                size: total,
                exact: false,
            });
            current = parent;
        }
        out
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
    fn a_listing_registers_nodes_children_size_and_purpose() {
        let index = FolderIndex::default();
        index.apply("/x", "/x", vec![entry("a")]);
        let children = index.children("/x");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "/x/a");
        assert!(index.set_purpose("/x/a", Some("cache".into())));
        index.apply_size("/x/a", 4096, true);
        let node = index.node("/x/a").unwrap();
        assert_eq!(node.size, Some(4096));
        assert!(node.exact);
        assert_eq!(node.purpose.as_deref(), Some("cache"));
        assert_eq!(node.parent.as_deref(), Some("/x"));
        index.remove_path("/x");
        assert!(index.node("/x/a").is_none());
    }

    #[test]
    fn scan_nodes_fill_the_same_tree_without_a_listing() {
        let index = FolderIndex::default();
        index.apply_scan_nodes(&[
            ScanNode {
                path: "/scan".into(),
                parent: None,
                is_dir: true,
                size: Some(10),
                exact: true,
            },
            ScanNode {
                path: "/scan/child".into(),
                parent: Some("/scan".into()),
                is_dir: true,
                size: Some(10),
                exact: true,
            },
        ]);
        let child = index.node("/scan/child").unwrap();
        assert_eq!(child.parent.as_deref(), Some("/scan"));
        assert_eq!(child.size, Some(10));
        assert!(child.exact);
        assert_eq!(index.children("/scan").len(), 1);
    }

    #[test]
    fn a_child_size_updates_its_parent() {
        let index = FolderIndex::default();
        index.apply(
            "/Users",
            "/Users",
            vec![
                FolderEntry {
                    name: "xiaocy".into(),
                    path: "/Users/xiaocy".into(),
                    is_dir: true,
                    size: 0,
                    modified_ms: None,
                },
                FolderEntry {
                    name: "Shared".into(),
                    path: "/Users/Shared".into(),
                    is_dir: true,
                    size: 0,
                    modified_ms: None,
                },
            ],
        );
        let receiver = index.subscribe_sizes();
        index.apply_size("/Users/xiaocy", 100, true);
        let parent = index.size("/Users");
        assert_eq!(parent.size, Some(100));
        assert!(!parent.exact);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_ok());
    }

    #[test]
    fn a_parent_listing_uses_a_child_size_that_arrived_first() {
        let index = FolderIndex::default();
        index.apply_size("/Users/xiaocy", 653_000, true);
        index.apply(
            "/Users",
            "/Users",
            vec![FolderEntry {
                name: "xiaocy".into(),
                path: "/Users/xiaocy".into(),
                is_dir: true,
                size: 0,
                modified_ms: None,
            }],
        );
        assert_eq!(index.size("/Users").size, Some(653_000));
        assert_eq!(index.node("/Users/xiaocy").unwrap().size, Some(653_000));
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
