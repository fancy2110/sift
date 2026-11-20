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
use crate::explorer::{DirSizeDto, MAX_ENTRIES_PER_WALK};

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

/// A monolithic JSON index above this size is not worth a blocking parse on
/// startup; it is moved aside and rebuilt incrementally.
const MAX_LOAD_BYTES: u64 = 64 * 1024 * 1024;

fn load_from_disk() -> TreeData {
    let Some(path) = tree_path() else {
        return TreeData::default();
    };
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > MAX_LOAD_BYTES => {
            let oversized = path.with_extension("json.oversized");
            let _ = std::fs::rename(&path, &oversized);
            return TreeData::default();
        }
        Ok(_) => {}
        Err(_) => return TreeData::default(),
    }
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
        // Keep persistence bounded: an unbounded JSON rewrite blocks the
        // writer thread and grows into hundreds of megabytes.
        if bytes.len() as u64 > MAX_LOAD_BYTES {
            return;
        }
        let _ = std::fs::write(&path, bytes);
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeDebitItem {
    pub path: String,
    pub bytes: u64,
}

/// Subtract bytes removed by a cleanup from paths and every ancestor.
///
/// Batched because one interactive cleanup can remove hundreds of items, and
/// each item must update the same in-memory tree.
#[tauri::command]
pub fn dir_size_debit(items: Vec<SizeDebitItem>) {
    if items.is_empty() {
        return;
    }
    let mut data = tree().lock().unwrap();
    for item in items {
        debit_one(&mut data, &item.path, item.bytes);
    }
    mark_dirty();
}

fn debit_one(data: &mut TreeData, path: &str, bytes: u64) {
    if bytes == 0 {
        return;
    }
    if let Some(size) = data.sizes.get_mut(path) {
        *size = size.saturating_sub(bytes);
    }
    let mut current = path.to_string();
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
}

/// Record that `parent` contains `child`, so updates can walk upward.
pub fn register_child(parent: &str, child: &str) {
    if parent.is_empty() || child.is_empty() || parent == child {
        return;
    }
    let Ok(mut data) = tree().try_lock() else {
        return;
    };
    data.parents.insert(child.to_string(), parent.to_string());
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
        // Never let a child-only sum replace a total measured by a full walk:
        // the parent may have direct files that are not child directories.
        if sum > 0 && !data.exact.get(&parent).copied().unwrap_or(false) {
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
    #[cfg(unix)]
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
}

/// Ingest results from the scan engine: every scanned directory contributes
/// its exact size, so a scan refreshes the shared index for all features.
#[tauri::command]
pub async fn scan_index_ingest(
    services: State<'_, AppServices>,
    entries: Vec<ScanEntryDto>,
) -> Result<(), String> {
    let store = services.store.clone();
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
        }
        // One upward pass per ingest batch: every touched ancestor becomes the
        // sum of its children.
        let paths: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
        for path in paths {
            recompute_ancestors(&mut data, &path);
        }
        drop(data);
        for entry in &entries {
            store
                .folders()
                .apply_size(&entry.path, entry.size, true);
        }
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
pub fn dir_size_peek(services: State<'_, AppServices>, paths: Vec<String>) -> Vec<DirSizeDto> {
    // Store first: a size task fills it, so every page can reuse the value
    // without touching the filesystem.
    let folders = services.store.folders();
    let Ok(data) = tree().try_lock() else {
        return paths
            .into_iter()
            .map(|path| {
                let view = folders.size(&path);
                if view.state != sift_store::ListingState::Missing {
                    DirSizeDto {
                        path,
                        size: view.size.unwrap_or(0),
                        partial: !view.exact,
                    }
                } else {
                    DirSizeDto {
                        path,
                        size: 0,
                        partial: true,
                    }
                }
            })
            .collect();
    };
    paths
        .into_iter()
        .map(|path| {
            let view = folders.size(&path);
            if view.state != sift_store::ListingState::Missing {
                return DirSizeDto {
                    path,
                    size: view.size.unwrap_or(0),
                    partial: !view.exact,
                };
            }
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
pub async fn dir_sizes_tree(
    services: State<'_, AppServices>,
    paths: Vec<String>,
    force: bool,
) -> Result<Vec<DirSizeDto>, String> {
    let store = services.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut measured = Vec::new();
        {
            let data = tree().lock().unwrap();
            for path in &paths {
                let known = data.sizes.get(path).copied().unwrap_or(0);
                let is_exact = data.exact.get(path).copied().unwrap_or(false);
                // A partial walk (permissions) must not freeze the value: only
                // exact totals are reused without re-measuring.
                if known > 0 && is_exact && !force {
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
        for item in &out {
            store
                .folders()
                .apply_size(&item.path, item.size, !item.partial);
        }
        out
    })
    .await
    .map_err(|error| error.to_string())
}

/// Drop the whole tree (manual rescan).
#[tauri::command]
pub fn dir_sizes_clear(services: State<'_, AppServices>) {
    services.store.folders().clear();
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
    fn an_exact_parent_is_not_shrunk_by_child_sums() {
        let mut data = sample();
        record(&mut data, "/a", 1_000, true);
        record(&mut data, "/a/b", 10, true);
        assert_eq!(data.sizes.get("/a"), Some(&1_000));
        assert_eq!(data.exact.get("/a"), Some(&true));
    }

    #[test]
    fn debiting_a_child_updates_its_ancestors() {
        let mut data = sample();
        record(&mut data, "/a/b", 100, true);
        record(&mut data, "/a", 100, true);
        debit_one(&mut data, "/a/b", 40);
        assert_eq!(data.sizes.get("/a/b"), Some(&60));
        assert_eq!(data.sizes.get("/a"), Some(&60));
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
