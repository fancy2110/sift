//! Authoritative directory-size tree.
//!
//! Sizes are computed and propagated here, not in the UI: every listing
//! registers parent/child edges, every measurement updates the node and
//! recomputes each ancestor as the sum of its children, and the tree is
//! persisted so the numbers survive a relaunch. Views only read it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::analyze::AppServices;
use crate::explorer::{DirEntryDto, DirSizeDto, MAX_ENTRIES_PER_WALK};

#[derive(Debug, Default, Serialize, Deserialize)]
struct TreeData {
    /// Recursive size in bytes for a path.
    sizes: HashMap<String, u64>,
    /// True when `sizes` came from a full walk; false for derived sums.
    exact: HashMap<String, bool>,
    /// Direct children of each directory.
    children: HashMap<String, HashSet<String>>,
    /// Direct parent of each path.
    parents: HashMap<String, String>,
    /// Basic metadata seen during listings.
    nodes: HashMap<String, NodeMeta>,
    /// Cached directory listings, keyed by the requested path.
    listings: HashMap<String, CachedListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedListing {
    at_ms: i64,
    resolved: String,
    entries: Vec<DirEntryDto>,
}

/// Listings older than this are re-read from disk.
const LISTING_TTL_MS: i64 = 5 * 60_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|age| age.as_millis() as i64)
        .unwrap_or(0)
}

/// Serve a listing from the shared index when it is still fresh.
pub fn cached_listing(path: &str) -> Option<(String, Vec<DirEntryDto>)> {
    let data = tree().try_lock().ok()?;
    let listing = data.listings.get(path)?;
    if now_ms() - listing.at_ms > LISTING_TTL_MS {
        return None;
    }
    Some((listing.resolved.clone(), listing.entries.clone()))
}

/// Remember a listing in the shared index.
pub fn store_listing(requested: &str, resolved: &str, entries: &[DirEntryDto]) {
    let Ok(mut data) = tree().try_lock() else {
        return;
    };
    data.listings.insert(
        requested.to_string(),
        CachedListing {
            at_ms: now_ms(),
            resolved: resolved.to_string(),
            entries: entries.to_vec(),
        },
    );
    mark_dirty();
}

/// Drop every cached listing (manual rescan).
#[tauri::command]
pub fn dir_listing_clear() {
    let mut data = tree().lock().unwrap();
    data.listings.clear();
    persist(&data);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeMeta {
    is_dir: bool,
    mtime_ms: Option<i64>,
}

static TREE: OnceLock<Mutex<TreeData>> = OnceLock::new();
/// Writes are debounced: a scan updates thousands of nodes per second and
/// serializing the whole tree each time would hold the lock and freeze the UI.
static DIRTY: AtomicBool = AtomicBool::new(false);
static FLUSHER: OnceLock<()> = OnceLock::new();

fn mark_dirty() {
    DIRTY.store(true, Ordering::Relaxed);
    FLUSHER.get_or_init(|| {
        std::thread::Builder::new()
            .name("sift-size-flush".into())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                if DIRTY.swap(false, Ordering::Relaxed) {
                    let data = tree().lock().unwrap();
                    persist(&data);
                }
            })
            .ok();
    });
}

fn tree_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|dir| dir.join("Sift").join("size-tree.json"))
}

fn load_from_disk() -> TreeData {
    let Some(path) = tree_path() else {
        return TreeData::default();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn tree() -> &'static Mutex<TreeData> {
    TREE.get_or_init(|| Mutex::new(load_from_disk()))
}

fn persist(data: &TreeData) {
    let Some(path) = tree_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(data) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Remember what a listing saw: kind and modification time.
pub fn register_node(path: &str, is_dir: bool, mtime_ms: Option<i64>) {
    // Never block a directory listing on the index: if a scan/flush holds it,
    // skip the bookkeeping for now.
    let Ok(mut data) = tree().try_lock() else {
        return;
    };
    data.nodes.insert(
        path.to_string(),
        NodeMeta {
            is_dir,
            mtime_ms,
        },
    );
}

/// Subtract bytes removed by a cleanup from a path and every ancestor.
#[tauri::command]
pub fn dir_size_debit(path: String, bytes: u64) {
    if bytes == 0 {
        return;
    }
    let mut data = tree().lock().unwrap();
    if let Some(size) = data.sizes.get_mut(&path) {
        *size = size.saturating_sub(bytes);
    }
    let mut current = path.clone();
    for _ in 0..64 {
        let Some(parent) = data.parents.get(&current).cloned() else {
            break;
        };
        if let Some(size) = data.sizes.get_mut(&parent) {
            *size = size.saturating_sub(bytes);
        } else {
            break;
        }
        current = parent;
    }
    // The entry is gone: drop it from every cached parent listing too.
    for listing in data.listings.values_mut() {
        listing
            .entries
            .retain(|entry| entry.path != path && !entry.path.starts_with(&format!("{path}/")));
    }
    mark_dirty();
}

/// Record that `parent` contains `child`, so updates can walk upward.
pub fn register_child(parent: &str, child: &str) {
    if parent.is_empty() || child.is_empty() || parent == child {
        return;
    }
    let Ok(mut data) = tree().try_lock() else {
        return;
    };
    data.parents
        .insert(child.to_string(), parent.to_string());
    data.children
        .entry(parent.to_string())
        .or_default()
        .insert(child.to_string());
    mark_dirty();
}

/// Recompute every ancestor of `path` as the sum of its known children.
fn recompute_ancestors(data: &mut TreeData, path: &str) {
    let mut current = path.to_string();
    for _ in 0..64 {
        let Some(parent) = data.parents.get(&current).cloned() else {
            break;
        };
        let Some(children) = data.children.get(&parent).cloned() else {
            break;
        };
        let mut sum = 0u64;
        for child in &children {
            sum = sum.saturating_add(data.sizes.get(child).copied().unwrap_or(0));
        }
        if sum > 0 {
            data.sizes.insert(parent.clone(), sum);
            data.exact.insert(parent.clone(), false);
        }
        current = parent;
    }
}

/// Record a measured size for `path` and update the tree upward.
fn record(data: &mut TreeData, path: &str, size: u64, exact: bool) {
    data.sizes.insert(path.to_string(), size);
    data.exact.insert(path.to_string(), exact);
    recompute_ancestors(data, path);
}

fn walk(path: &str) -> (u64, bool) {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let mut size = 0u64;
    let mut visited = 0u64;
    let mut partial = false;
    // Count a hardlinked file once, like Finder and du.
    #[cfg(any(unix, windows))]
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut stack = vec![std::path::PathBuf::from(path)];
    while let Some(dir) = stack.pop() {
        let Ok(reader) = std::fs::read_dir(&dir) else {
            partial = true;
            continue;
        };
        for item in reader.flatten() {
            visited += 1;
            if visited > MAX_ENTRIES_PER_WALK {
                partial = true;
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(item.path());
            } else if meta.is_file() {
                #[cfg(unix)]
                if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
                    continue;
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if let (Some(volume), Some(index)) =
                        (meta.volume_serial_number(), meta.file_index())
                    {
                        if !seen.insert((volume as u64, index)) {
                            continue;
                        }
                    }
                }
                size = size.saturating_add(crate::explorer::size_of(item.path().as_path(), &meta));
            }
        }
    }
    (size, !partial)
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanEntryDto {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub mtime_ms: Option<i64>,
}

/// Ingest results from the scan engine: every scanned directory contributes
/// its exact size, so a scan refreshes the shared index for all features.
#[tauri::command]
pub async fn scan_index_ingest(entries: Vec<ScanEntryDto>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if entries.is_empty() {
            return;
        }
        let Ok(mut data) = tree().lock() else {
            return;
        };
    for entry in &entries {
        if !std::path::Path::new(&entry.path).is_absolute() {
            continue;
        }
        if entry.is_dir {
            if let Some(parent) = std::path::Path::new(&entry.path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
            {
                if !parent.is_empty() && parent != entry.path {
                    data.parents.insert(entry.path.clone(), parent.clone());
                    data.children
                        .entry(parent)
                        .or_default()
                        .insert(entry.path.clone());
                }
            }
            if entry.size > 0 {
                data.sizes.insert(entry.path.clone(), entry.size);
                data.exact.insert(entry.path.clone(), true);
            }
        }
        data.nodes.insert(
            entry.path.clone(),
            NodeMeta {
                is_dir: entry.is_dir,
                mtime_ms: entry.mtime_ms,
            },
        );
    }
        // One upward pass per ingest batch: every touched ancestor becomes the
        // sum of its children.
        let paths: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
        for path in paths {
            recompute_ancestors(&mut data, &path);
        }
        drop(data);
        mark_dirty();
    })
    .await
    .map_err(|error| error.to_string())
}

/// Live update from the scan engine: a directory finished measuring.
pub fn record_scan_dir(path: &str, size: u64) {
    if !std::path::Path::new(path).is_absolute() || size == 0 {
        return;
    }
    if let Some(parent) = std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
    {
        if !parent.is_empty() && parent != path {
            register_child(&parent, path);
        }
    }
    let mut data = tree().lock().unwrap();
    record(&mut data, path, size, true);
}

/// Cached totals only (no walking); used to paint instantly.
#[tauri::command]
pub fn dir_size_peek(paths: Vec<String>) -> Vec<DirSizeDto> {
    // Sync command on the main thread: never block on the index.
    let Ok(data) = tree().try_lock() else {
        return paths
            .into_iter()
            .map(|path| DirSizeDto {
                path,
                size: 0,
                partial: true,
            })
            .collect();
    };
    paths
        .into_iter()
        .map(|path| {
            let size = data.sizes.get(&path).copied().unwrap_or(0);
            let partial = !data.exact.get(&path).copied().unwrap_or(true);
            DirSizeDto {
                path,
                size,
                partial,
            }
        })
        .collect()
}

/// Measure missing folders (or all when `force`), updating the tree upward.
#[tauri::command]
pub async fn dir_sizes_tree(paths: Vec<String>, force: bool) -> Vec<DirSizeDto> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut measured = Vec::new();
        {
            let data = tree().lock().unwrap();
            for path in &paths {
                let known = data.sizes.get(path).copied().unwrap_or(0);
                if known > 0 && !force {
                    out.push(DirSizeDto {
                        path: path.clone(),
                        size: known,
                        partial: !data.exact.get(path).copied().unwrap_or(false),
                    });
                } else {
                    measured.push(path.clone());
                }
            }
        }
        let results: Vec<(String, u64, bool)> = measured
            .into_iter()
            .map(|path| {
                let (size, full) = walk(&path);
                (path, size, full)
            })
            .collect();
        {
            let mut data = tree().lock().unwrap();
            for (path, size, full) in &results {
                record(&mut data, path, *size, *full);
            }
            mark_dirty();
            for (path, _, _) in &results {
                out.push(DirSizeDto {
                    path: path.clone(),
                    size: data.sizes.get(path).copied().unwrap_or(0),
                    partial: !data.exact.get(path).copied().unwrap_or(false),
                });
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Drop the whole tree (manual rescan).
#[tauri::command]
pub fn dir_sizes_clear(_services: State<'_, AppServices>) {
    let mut data = tree().lock().unwrap();
    *data = TreeData::default();
    persist(&data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TreeData {
        let mut data = TreeData::default();
        for (child, parent) in [("/a/b", "/a"), ("/a/c", "/a"), ("/a", "/")] {
            data.parents.insert(child.into(), parent.into());
            data.children
                .entry(parent.to_string())
                .or_default()
                .insert(child.to_string());
        }
        data
    }

    #[test]
    fn parents_are_the_sum_of_their_children() {
        let mut data = sample();
        record(&mut data, "/a/b", 100, true);
        record(&mut data, "/a/c", 50, true);
        record(&mut data, "/a", 150, true);
        assert_eq!(data.sizes.get("/a"), Some(&150));
        assert_eq!(data.sizes.get("/"), Some(&150));
        assert_eq!(data.exact.get("/"), Some(&false));
    }

    #[test]
    fn a_late_child_updates_every_ancestor() {
        let mut data = sample();
        record(&mut data, "/a/b", 10, true);
        record(&mut data, "/a/c", 10, true);
        assert_eq!(data.sizes.get("/"), Some(&20));
        // The child turns out to be much larger than the first measurement.
        record(&mut data, "/a/c", 10_000, true);
        assert_eq!(data.sizes.get("/a"), Some(&10_010));
        assert_eq!(data.sizes.get("/"), Some(&10_010));
    }
}
