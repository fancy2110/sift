//! The Store-owned folder index.
//!
//! File lists are a read model, not a scan by-product: views ask this index for
//! a listing, and a miss returns `Missing` instead of touching the filesystem.
//! The I/O side (fetch queue) fills the index and publishes a revision; every
//! consumer can subscribe and refresh from the same snapshot.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

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
    /// True only when a Store size task recursively measured this path. Scan
    /// sizes are useful immediately but must be verified before they suppress
    /// a background measurement (a permission-limited scan can under-count).
    verified: bool,
    at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeView {
    pub path: String,
    pub size: Option<u64>,
    pub exact: bool,
    pub verified: bool,
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
    snapshot_dirty: AtomicBool,
    last_snapshot_ms: AtomicU64,
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
            snapshot_dirty: AtomicBool::new(false),
            last_snapshot_ms: AtomicU64::new(0),
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
        self.snapshot_dirty.store(true, Ordering::Relaxed);
        // A parent's total is the sum of its direct children (including
        // direct files). Recompute it as soon as the listing supplies edges;
        // no exact-parent exception, because the UI must agree with the rows.
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
            self.apply_derived_size(requested, known);
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
                        verified: true,
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
        // Keep lock scopes disjoint. `children()` locks children then node,
        // while this path may run concurrently with a UI query.
        {
            let mut tree = self.nodes.lock().unwrap();
            for node in nodes {
                tree.entry(node.path.clone())
                    .and_modify(|record| record.is_dir = node.is_dir)
                    .or_insert(NodeRecord {
                        parent: node.parent.clone(),
                        is_dir: node.is_dir,
                        purpose: None,
                    });
            }
        }
        {
            let mut children = self.children.lock().unwrap();
            for node in nodes {
                if let Some(parent) = &node.parent {
                    children
                        .entry(parent.clone())
                        .or_default()
                        .insert(node.path.clone());
                }
            }
        }
        {
            let mut sizes = self.sizes.lock().unwrap();
            for node in nodes {
                if let Some(size) = node.size {
                    sizes.insert(
                        node.path.clone(),
                        SizeRecord {
                            size,
                            exact: node.exact,
                            verified: false,
                            at_ms: now_ms(),
                        },
                    );
                }
            }
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.size_revision.fetch_add(1, Ordering::AcqRel);
        self.snapshot_dirty.store(true, Ordering::Relaxed);
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
                self.snapshot_dirty.store(true, Ordering::Relaxed);
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
        self.snapshot_dirty.store(true, Ordering::Relaxed);
    }

    /// A path disappeared: drop its listing and size, and publish the removal
    /// as a zero-sized, non-exact record so views can prune immediately.
    pub fn remove_path(&self, requested: &str) {
        self.snapshot_dirty.store(true, Ordering::Relaxed);
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
        self.snapshot_dirty.store(true, Ordering::Relaxed);
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
                verified: false,
                state: ListingState::Missing,
                revision,
            },
            Some(record) => SizeView {
                path: requested.to_string(),
                size: Some(record.size),
                exact: record.exact,
                verified: record.verified,
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
                verified: false,
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
        self.snapshot_dirty.store(true, Ordering::Relaxed);
        change
    }

    /// A parent total derived from known children. It is useful for display
    /// but is not a recursive measurement, so it must not suppress a size task.
    pub fn apply_derived_size(&self, path: &str, size: u64) -> SizeChange {
        self.sizes.lock().unwrap().insert(
            path.to_string(),
            SizeRecord {
                size,
                exact: false,
                verified: false,
                at_ms: now_ms(),
            },
        );
        let revision = self.size_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let change = SizeChange {
            revision,
            path: path.to_string(),
            size,
            exact: false,
        };
        let mut changes = vec![change.clone()];
        changes.extend(self.recompute_size_ancestors(path, revision));
        let mut subscribers = self.size_subscribers.lock().unwrap();
        for change in changes {
            subscribers.retain(|sender| sender.send(change.clone()).is_ok());
        }
        self.snapshot_dirty.store(true, Ordering::Relaxed);
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
                verified: true,
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
        self.snapshot_dirty.store(true, Ordering::Relaxed);
        change
    }

    /// Walk upward from a changed child. An exact ancestor already includes
    /// its children and is left untouched; a missing/derived ancestor becomes
    /// the sum of all currently known direct children.
    fn recompute_size_ancestors(&self, path: &str, revision: u64) -> Vec<SizeChange> {
        // Hold both maps for the short upward walk. Cloning `children` here
        // would make every size update O(number of directories).
        let children = self.children.lock().unwrap();
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
                    verified: false,
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

    /// Rebuild every directory total from its direct children. Called after a
    /// scan ingest so a full scan's exact parent numbers cannot disagree with
    /// the child rows; the UI wants parent = sum(children), not a mixed source.
    pub fn recompute_all_sizes(&self) -> usize {
        let children = self.children.lock().unwrap().clone();
        let mut parents: Vec<(usize, String)> = children
            .keys()
            .map(|path| (path.matches('/').count(), path.clone()))
            .collect();
        parents.sort_by(|a, b| b.0.cmp(&a.0));
        let mut sizes = self.sizes.lock().unwrap();
        let mut changes = Vec::new();
        for (_, parent) in parents {
            let Some(kids) = children.get(&parent) else {
                continue;
            };
            if !kids.iter().any(|child| sizes.contains_key(child)) {
                continue;
            }
            let total: u64 = kids
                .iter()
                .filter_map(|child| sizes.get(child).map(|record| record.size))
                .sum();
            let changed = sizes
                .get(&parent)
                .map(|record| record.size != total || record.exact)
                .unwrap_or(true);
            sizes.insert(
                parent.clone(),
                SizeRecord {
                    size: total,
                    exact: false,
                    verified: false,
                    at_ms: now_ms(),
                },
            );
            if changed {
                changes.push(parent);
            }
        }
        if !changes.is_empty() {
            let revision = self.size_revision.fetch_add(1, Ordering::AcqRel) + 1;
            let mut subscribers = self.size_subscribers.lock().unwrap();
            for path in &changes {
                let size = sizes.get(path).map(|record| record.size).unwrap_or(0);
                subscribers.retain(|sender| {
                    sender
                        .send(SizeChange {
                            revision,
                            path: path.clone(),
                            size,
                            exact: false,
                        })
                        .is_ok()
                });
            }
            self.snapshot_dirty.store(true, Ordering::Relaxed);
        }
        changes.len()
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
    // ---- compact snapshot -------------------------------------------------

    /// Serialize the read model into a compressed, path-deduplicating binary
    /// snapshot. Listings store names, not full child paths; sizes/purposes
    /// are stored once. A 150k-directory index is a few MB instead of the
    /// tens-of-MB JSON document it replaces.
    pub fn snapshot_bytes(&self) -> Result<Vec<u8>, String> {
        let listings = self.listings.lock().unwrap().clone();
        let sizes = self.sizes.lock().unwrap().clone();
        let nodes = self.nodes.lock().unwrap().clone();
        let mut raw = Vec::with_capacity(1024 * 1024);
        raw.extend_from_slice(b"SFTREE02");
        write_u32(&mut raw, listings.len() as u32);
        let mut ordered: Vec<(&String, &FolderListing)> = listings.iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(b.0));
        for (path, listing) in ordered {
            write_str(&mut raw, path)?;
            write_str(&mut raw, &listing.resolved)?;
            write_i64(&mut raw, listing.at_ms);
            write_u32(&mut raw, listing.entries.len() as u32);
            for entry in &listing.entries {
                write_str(&mut raw, &entry.name)?;
                raw.push(entry.is_dir as u8);
                write_u64(&mut raw, entry.size);
                match entry.modified_ms {
                    Some(value) => {
                        raw.push(1);
                        write_i64(&mut raw, value);
                    }
                    None => raw.push(0),
                }
            }
        }
        let mut sizes_ordered: Vec<(&String, &SizeRecord)> = sizes.iter().collect();
        sizes_ordered.sort_by(|a, b| a.0.cmp(b.0));
        write_u32(&mut raw, sizes_ordered.len() as u32);
        for (path, record) in sizes_ordered {
            write_str(&mut raw, path)?;
            write_u64(&mut raw, record.size);
            raw.push(record.exact as u8);
            raw.push(record.verified as u8);
            write_i64(&mut raw, record.at_ms);
        }
        let purposes: Vec<(&String, &String)> = nodes
            .iter()
            .filter_map(|(path, node)| node.purpose.as_ref().map(|purpose| (path, purpose)))
            .collect();
        write_u32(&mut raw, purposes.len() as u32);
        for (path, purpose) in purposes {
            write_str(&mut raw, path)?;
            write_str(&mut raw, purpose)?;
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&raw).map_err(|error| error.to_string())?;
        encoder.finish().map_err(|error| error.to_string())
    }

    /// Restore a snapshot produced by [`snapshot_bytes`]. All current in-memory
    /// state is replaced atomically only after the snapshot decodes.
    pub fn restore_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        let mut decoder = GzDecoder::new(bytes);
        let mut raw = Vec::new();
        decoder.read_to_end(&mut raw).map_err(|error| error.to_string())?;
        if raw.len() < 8 || &raw[..8] != b"SFTREE02" {
            return Err("tree snapshot: bad magic".into());
        }
        let mut cursor = 8usize;
        let listing_count = read_u32(&raw, &mut cursor)? as usize;
        let mut listings = HashMap::with_capacity(listing_count);
        for _ in 0..listing_count {
            let path = read_str(&raw, &mut cursor)?;
            let resolved = read_str(&raw, &mut cursor)?;
            let at_ms = read_i64(&raw, &mut cursor)?;
            let entry_count = read_u32(&raw, &mut cursor)? as usize;
            let mut entries = Vec::with_capacity(entry_count);
            for _ in 0..entry_count {
                let name = read_str(&raw, &mut cursor)?;
                let is_dir = read_u8(&raw, &mut cursor)? != 0;
                let size = read_u64(&raw, &mut cursor)?;
                let modified_ms = if read_u8(&raw, &mut cursor)? != 0 {
                    Some(read_i64(&raw, &mut cursor)?)
                } else {
                    None
                };
                let child = child_path(&path, &name);
                entries.push(FolderEntry {
                    name,
                    path: child,
                    is_dir,
                    size,
                    modified_ms,
                });
            }
            listings.insert(
                path.clone(),
                FolderListing {
                    resolved,
                    entries,
                    at_ms,
                },
            );
        }
        let size_count = read_u32(&raw, &mut cursor)? as usize;
        let mut sizes = HashMap::with_capacity(size_count);
        for _ in 0..size_count {
            let path = read_str(&raw, &mut cursor)?;
            sizes.insert(
                path,
                SizeRecord {
                    size: read_u64(&raw, &mut cursor)?,
                    exact: read_u8(&raw, &mut cursor)? != 0,
                    verified: read_u8(&raw, &mut cursor)? != 0,
                    at_ms: read_i64(&raw, &mut cursor)?,
                },
            );
        }
        let purpose_count = read_u32(&raw, &mut cursor)? as usize;
        let mut purposes = HashMap::with_capacity(purpose_count);
        for _ in 0..purpose_count {
            purposes.insert(
                read_str(&raw, &mut cursor)?,
                read_str(&raw, &mut cursor)?,
            );
        }

        // Build the tree edges from the restored listings.
        let mut nodes: HashMap<String, NodeRecord> = HashMap::new();
        let mut children: HashMap<String, HashSet<String>> = HashMap::new();
        for (path, listing) in &listings {
            let parent = std::path::Path::new(path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.to_string_lossy().into_owned());
            nodes.entry(path.clone()).or_insert(NodeRecord {
                parent,
                is_dir: true,
                purpose: purposes.get(path).cloned(),
            });
            let mut set = HashSet::with_capacity(listing.entries.len());
            for entry in &listing.entries {
                nodes.entry(entry.path.clone()).or_insert(NodeRecord {
                    parent: Some(path.clone()),
                    is_dir: entry.is_dir,
                    purpose: purposes.get(&entry.path).cloned(),
                });
                set.insert(entry.path.clone());
            }
            children.insert(path.clone(), set);
        }
        for (path, purpose) in purposes {
            nodes.entry(path.clone()).or_insert(NodeRecord {
                parent: std::path::Path::new(&path)
                    .parent()
                    .map(|parent| parent.to_string_lossy().into_owned()),
                is_dir: true,
                purpose: Some(purpose),
            });
        }

        *self.listings.lock().unwrap() = listings;
        *self.nodes.lock().unwrap() = nodes;
        *self.children.lock().unwrap() = children;
        *self.sizes.lock().unwrap() = sizes;
        self.revision.store(0, Ordering::Release);
        self.size_revision.store(0, Ordering::Release);
        self.snapshot_dirty.store(false, Ordering::Relaxed);
        self.last_snapshot_ms.store(now_ms() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Atomic snapshot write to `path` (temp file + rename).
    pub fn snapshot_to(&self, path: &std::path::Path) -> std::io::Result<bool> {
        let bytes = self
            .snapshot_bytes()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, path)?;
        self.snapshot_dirty.store(false, Ordering::Relaxed);
        self.last_snapshot_ms.store(now_ms() as u64, Ordering::Relaxed);
        Ok(true)
    }

    /// Restore if the snapshot exists. An absent file is not an error.
    pub fn restore_from(&self, path: &std::path::Path) -> std::io::Result<bool> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        self.restore_bytes(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        Ok(true)
    }

    /// Write at most once per 30 seconds unless `force`.
    pub fn snapshot_if_dirty(&self, path: &std::path::Path, force: bool) -> std::io::Result<bool> {
        if !self.snapshot_dirty.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let now = now_ms() as u64;
        if !force && now.saturating_sub(self.last_snapshot_ms.load(Ordering::Relaxed)) < 30_000 {
            return Ok(false);
        }
        self.snapshot_to(path)
    }

    pub fn snapshot_dirty(&self) -> bool {
        self.snapshot_dirty.load(Ordering::Relaxed)
    }

}


fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}


fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn write_str(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    let len = u32::try_from(bytes.len()).map_err(|_| "tree snapshot: string too long")?;
    write_u32(out, len);
    out.extend_from_slice(bytes);
    Ok(())
}
fn read_u8(raw: &[u8], cursor: &mut usize) -> Result<u8, String> {
    let value = *raw.get(*cursor).ok_or("tree snapshot: truncated")?;
    *cursor += 1;
    Ok(value)
}
fn read_u32(raw: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = cursor.checked_add(4).ok_or("tree snapshot: overflow")?;
    let bytes: [u8; 4] = raw
        .get(*cursor..end)
        .ok_or("tree snapshot: truncated")?
        .try_into()
        .map_err(|_| "tree snapshot: truncated")?;
    *cursor = end;
    Ok(u32::from_le_bytes(bytes))
}
fn read_u64(raw: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let end = cursor.checked_add(8).ok_or("tree snapshot: overflow")?;
    let bytes: [u8; 8] = raw
        .get(*cursor..end)
        .ok_or("tree snapshot: truncated")?
        .try_into()
        .map_err(|_| "tree snapshot: truncated")?;
    *cursor = end;
    Ok(u64::from_le_bytes(bytes))
}
fn read_i64(raw: &[u8], cursor: &mut usize) -> Result<i64, String> {
    read_u64(raw, cursor).map(|value| value as i64)
}
fn read_str(raw: &[u8], cursor: &mut usize) -> Result<String, String> {
    let len = read_u32(raw, cursor)? as usize;
    if len > 1024 * 1024 {
        return Err("tree snapshot: string too long".into());
    }
    let end = cursor.checked_add(len).ok_or("tree snapshot: overflow")?;
    let bytes = raw.get(*cursor..end).ok_or("tree snapshot: truncated")?;
    *cursor = end;
    String::from_utf8(bytes.to_vec()).map_err(|_| "tree snapshot: invalid utf8".into())
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
    fn a_snapshot_round_trips_listings_sizes_and_purpose() {
        let index = FolderIndex::default();
        index.apply("/x", "/x", vec![entry("a")]);
        index.apply_size("/x/a", 4096, true);
        index.set_purpose("/x/a", Some("cache".into()));
        let bytes = index.snapshot_bytes().unwrap();
        let restored = FolderIndex::default();
        restored.restore_bytes(&bytes).unwrap();
        assert_eq!(restored.list("/x").entries[0].name, "a");
        assert_eq!(restored.size("/x/a").size, Some(4096));
        assert_eq!(restored.node("/x/a").unwrap().purpose.as_deref(), Some("cache"));
        assert!(!restored.snapshot_dirty());
    }

    #[test]
    fn a_parent_is_the_sum_of_its_children_even_after_an_exact_total() {
        let index = FolderIndex::default();
        index.apply(
            "/sum",
            "/sum",
            vec![
                FolderEntry {
                    name: "file".into(),
                    path: "/sum/file".into(),
                    is_dir: false,
                    size: 10,
                    modified_ms: None,
                },
                FolderEntry {
                    name: "dir".into(),
                    path: "/sum/dir".into(),
                    is_dir: true,
                    size: 0,
                    modified_ms: None,
                },
            ],
        );
        index.apply_size("/sum", 1000, true);
        index.apply_size("/sum/dir", 5, true);
        assert_eq!(index.size("/sum").size, Some(15));
        assert!(!index.size("/sum").exact);
    }

    #[test]
    fn recompute_all_overwrites_exact_parents_with_child_sums() {
        let index = FolderIndex::default();
        index.apply(
            "/sum",
            "/sum",
            vec![
                FolderEntry {
                    name: "file".into(),
                    path: "/sum/file".into(),
                    is_dir: false,
                    size: 10,
                    modified_ms: None,
                },
                FolderEntry {
                    name: "dir".into(),
                    path: "/sum/dir".into(),
                    is_dir: true,
                    size: 0,
                    modified_ms: None,
                },
            ],
        );
        index.apply_size("/sum/dir", 5, true);
        index.apply_size("/sum", 1000, true);
        assert_eq!(index.size("/sum").size, Some(1000));
        index.recompute_all_sizes();
        assert_eq!(index.size("/sum").size, Some(15));
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
    fn files_in_a_listing_are_persisted_and_summed() {
        let index = FolderIndex::default();
        index.apply(
            "/x",
            "/x",
            vec![
                FolderEntry {
                    name: "file.bin".into(),
                    path: "/x/file.bin".into(),
                    is_dir: false,
                    size: 100,
                    modified_ms: Some(1),
                },
                entry("dir"),
            ],
        );
        index.apply_size("/x/dir", 50, true);
        assert_eq!(index.node("/x/file.bin").unwrap().size, Some(100));
        assert_eq!(index.list("/x").entries.len(), 2);
        assert_eq!(index.size("/x").size, Some(150));
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
