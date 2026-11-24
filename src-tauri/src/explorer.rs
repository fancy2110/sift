//! On-demand directory browsing, independent of a scan.
//!
//! The Explorer must work before (or without) a full scan: it lists one
//! directory at a time and computes child-folder totals lazily. Sizes are
//! bounded per directory so a huge folder cannot pin the UI forever.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::analyze::AppServices;
use crate::fetch_queue::{
    FetchQueue, TaskKind, PRIORITY_ACTIVE, PRIORITY_STALE, PRIORITY_VISIBLE,
};

/// Immediate children of one directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// File size for files; 0 for directories until `dir_sizes` fills it.
    pub size: u64,
    /// Last modification time in Unix ms; `None` when unavailable.
    pub modified_ms: Option<i64>,
}

/// One file below a directory (recursive listing for the right pane).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    /// Path relative to the listed directory, using `/` separators.
    pub relative: String,
    pub name: String,
    pub path: String,
    pub size: u64,
}

/// One directory listing with the resolved absolute path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListingDto {
    pub path: String,
    pub entries: Vec<DirEntryDto>,
    /// True when the Store has no fresh listing yet; the fetch queue is
    /// filling it and will publish `store://folders-changed`.
    #[serde(default)]
    pub pending: bool,
}

/// Recursive total for one directory (or one file).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirSizeDto {
    pub path: String,
    pub size: u64,
    /// True when the walk hit the per-directory cap.
    pub partial: bool,
}

/// Upper bound on entries visited per directory during a size walk.
pub(crate) const MAX_ENTRIES_PER_WALK: u64 = 20_000_000;

fn resolve(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    let path = if trimmed.is_empty() {
        dirs::home_dir().ok_or_else(|| "err.noHome".to_string())?
    } else {
        PathBuf::from(trimmed)
    };
    if !path.is_absolute() {
        return Err("err.pathNotAbsolute".to_string());
    }
    Ok(path)
}

/// Overlay Store totals onto a listing. Directory rows arrive with size 0;
/// the Store may already know the total from a scan or a size task.
fn entries_with_store_sizes(
    folders: &sift_store::FolderIndex,
    entries: Vec<sift_store::FolderEntry>,
) -> Vec<DirEntryDto> {
    entries
        .into_iter()
        .map(|entry| DirEntryDto {
            name: entry.name,
            path: entry.path.clone(),
            is_dir: entry.is_dir,
            size: if entry.is_dir {
                // Keep listing construction bounded: never parse the legacy
                // size index here. A missing/derived total is filled by the
                // parallel size pass after the list is painted.
                folders.size(&entry.path).size.unwrap_or(entry.size)
            } else {
                entry.size
            },
            modified_ms: entry.modified_ms,
        })
        .collect()
}

#[tauri::command]
pub async fn list_dir_entries(
    services: State<'_, AppServices>,
    queue: State<'_, FetchQueue>,
    path: String,
    force: Option<bool>,
) -> Result<DirListingDto, String> {
    // Store is the read model: never read the filesystem here. A miss/stale
    // listing is queued and an empty (or stale) view is returned immediately.
    let requested = resolve(&path)?.to_string_lossy().into_owned();
    let folders = services.store.folders();
    if force.unwrap_or(false) {
        folders.invalidate(&requested);
    }
    let view = folders.list(&requested);
    let pending = match view.state {
        sift_store::ListingState::Fresh => false,
        sift_store::ListingState::Stale | sift_store::ListingState::Missing => {
            queue.enqueue_list(&requested, PRIORITY_VISIBLE);
            true
        }
    };
    let entries = entries_with_store_sizes(&folders, view.entries);
    // The listing is cheap; a directory without a fresh Store total is the
    // missing data that belongs on the size queue, not a synchronous walk.
    for entry in &entries {
        if !entry.is_dir {
            continue;
        }
        let view = folders.size(&entry.path);
        // A derived total is only a partial sum (direct files + already-known
        // children); it must still be measured recursively.
        if view.state != sift_store::ListingState::Fresh || !view.verified {
            let priority = if view.state == sift_store::ListingState::Missing || !view.verified {
                PRIORITY_ACTIVE
            } else {
                PRIORITY_STALE
            };
            queue.enqueue(&entry.path, TaskKind::Size, priority);
        }
    }
    Ok(DirListingDto {
        path: view.resolved,
        entries,
        pending,
    })
}

/// One node from the Store tree, for pages that need structure rather than a
/// single directory listing.
#[tauri::command]
pub fn store_node(
    services: State<'_, AppServices>,
    path: String,
) -> Option<sift_store::NodeView> {
    services.store.folders().node(&path)
}

/// Direct children recorded in the Store tree.
#[tauri::command]
pub fn store_children(
    services: State<'_, AppServices>,
    path: String,
) -> Vec<sift_store::NodeView> {
    services.store.folders().children(&path)
}

/// Drop every folder listing so the next read is a miss (manual rescan).
#[tauri::command]
pub fn folders_clear(services: State<'_, AppServices>) {
    services.store.folders().clear();
}

/// Raw OS read for the fetch queue. Deliberately not a Tauri command: only the
/// queue may call it, so the Store remains the only read surface for views.
pub(crate) fn read_entries(path: &str) -> Result<DirListingDto, String> {
    let dir = resolve(path)?;
    let reader = std::fs::read_dir(&dir).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for item in reader.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let entry_path = item.path();
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
            continue;
        };
        // A symlinked directory is shown as a file-like entry: following it
        // could leave the subtree the user is browsing.
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        // Folder listings belong to the Store. The size index is updated by
        // its own scan/measure paths, not by a listing read.
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|age| age.as_millis() as i64);
        entries.push(DirEntryDto {
            name,
            path: entry_path.to_string_lossy().into_owned(),
            is_dir,
            size,
            modified_ms,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(DirListingDto {
        path: dir.to_string_lossy().into_owned(),
        entries,
        pending: false,
    })
}

#[tauri::command]
pub async fn dir_sizes(paths: Vec<String>) -> Vec<DirSizeDto> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| recursive_size(&path))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Finder reports allocated bytes; APFS compression makes `len()` larger.
#[cfg(windows)]
extern "system" {
    /// Allocated size on the volume, including compression/sparse handling.
    fn GetCompressedFileSizeW(file: *const u16, high: *mut u32) -> u32;
}

#[cfg(windows)]
fn allocated_windows(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut high: u32 = 0;
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    // 0xFFFFFFFF without a high part means INVALID_FILE_SIZE.
    if low == u32::MAX && high == 0 {
        return None;
    }
    Some(((high as u64) << 32) | low as u64)
}

pub(crate) fn size_of(path: &std::path::Path, meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        let _ = path;
        use std::os::unix::fs::MetadataExt;
        let blocks = meta.blocks();
        if blocks > 0 {
            return blocks.saturating_mul(512);
        }
    }
    #[cfg(windows)]
    if let Some(size) = allocated_windows(path) {
        return size;
    }
    meta.len()
}

pub(crate) fn recursive_size(path: &str) -> DirSizeDto {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let mut size = 0u64;
    let mut visited = 0u64;
    let mut partial = false;
    // Hardlinked files consume one copy on disk; Finder (and du) counts them
    // once. Only multi-link files need tracking, which keeps this small.
    #[cfg(unix)]
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut stack: Vec<PathBuf> = vec![Path::new(path).to_path_buf()];

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
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(item.path());
            } else {
                #[cfg(unix)]
                if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
                    continue;
                }
                size = size.saturating_add(size_of(&item.path(), &meta));
            }
        }
        if partial && visited > MAX_ENTRIES_PER_WALK {
            break;
        }
    }

    DirSizeDto {
        path: path.to_string(),
        size,
        partial,
    }
}

/// Upper bound on files returned for the right pane.
const MAX_FILES_PER_LIST: usize = 2_000;

#[tauri::command]
pub async fn list_files_under(
    path: String,
    limit: Option<usize>,
) -> Result<Vec<FileEntryDto>, String> {
    let limit = limit.unwrap_or(MAX_FILES_PER_LIST).min(MAX_FILES_PER_LIST);
    tauri::async_runtime::spawn_blocking(move || list_files(&path, limit))
        .await
        .map_err(|error| error.to_string())?
}

fn list_files(path: &str, limit: usize) -> Result<Vec<FileEntryDto>, String> {
    let root = resolve(path)?;
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in reader.flatten() {
            if files.len() >= limit {
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(item.path());
                continue;
            }
            let path = item.path();
            let relative = path
                .strip_prefix(&root)
                .map(|rest| {
                    rest.to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                })
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            files.push(FileEntryDto {
                relative,
                name: item.file_name().to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                size: meta.len(),
            });
        }
    }
    files.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.relative.cmp(&b.relative))
    });
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sift-explorer-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/nested")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/one.txt"), b"12345").unwrap();
        std::fs::write(root.join("a/nested/two.txt"), b"123").unwrap();
        std::fs::write(root.join("b/three.txt"), b"1234567").unwrap();
        std::fs::write(root.join("top.txt"), b"1").unwrap();
        root
    }

    #[test]
    fn lists_directories_first_and_sizes_files() {
        let root = fixture("list");
        let listing = read_entries(root.to_str().unwrap()).unwrap();
        let entries = &listing.entries;
        assert_eq!(entries[0].name, "a");
        assert!(entries[0].is_dir);
        let top = entries
            .iter()
            .find(|entry| entry.name == "top.txt")
            .unwrap();
        assert_eq!(top.size, 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recursive_size_sums_without_following_links() {
        let root = fixture("size");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("b"), root.join("a/link")).unwrap();
        let total = recursive_size(root.join("a").to_str().unwrap());
        // Physical allocation per file (blocks), not logical bytes; the
        // symlink's target must not be counted.
        let expected = [root.join("a/one.txt"), root.join("a/nested/two.txt")]
            .iter()
            .map(|path| size_of(path, &std::fs::metadata(path).unwrap()))
            .sum::<u64>();
        assert_eq!(total.size, expected, "symlink skipped");
        assert!(!total.partial);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn files_are_listed_recursively_with_relative_paths() {
        let root = fixture("files");
        let files = list_files(root.to_str().unwrap(), 100).unwrap();
        assert_eq!(files.len(), 4);
        assert!(files.iter().any(|file| file.relative == "a/nested/two.txt"));
        assert!(files.iter().any(|file| file.name == "top.txt"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    #[ignore]
    fn walk_probe() {
        let path = std::env::var("SIFT_WALK_PATH").expect("SIFT_WALK_PATH");
        let started = std::time::Instant::now();
        let dto = recursive_size(&path);
        println!(
            "size={} partial={} elapsed_ms={}",
            dto.size,
            dto.partial,
            started.elapsed().as_millis()
        );
    }

    #[test]
    fn a_raw_listing_can_fill_the_store_index() {
        let root = fixture("store-fill");
        let listing = read_entries(root.to_str().unwrap()).unwrap();
        let index = sift_store::FolderIndex::default();
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
        let change = index.apply(root.to_str().unwrap(), &listing.path, entries);
        assert_eq!(change.revision, 1);
        let view = index.list(root.to_str().unwrap());
        assert_eq!(view.state, sift_store::ListingState::Fresh);
        assert!(!view.entries.is_empty());
    }

    #[test]
    fn a_parent_listing_overlays_child_totals_from_store() {
        let index = sift_store::FolderIndex::default();
        index.apply(
            "/__sift_store_test__",
            "/__sift_store_test__",
            vec![sift_store::FolderEntry {
                name: "child".into(),
                path: "/__sift_store_test__/child".into(),
                is_dir: true,
                size: 0,
                modified_ms: None,
            }],
        );
        index.apply_size("/__sift_store_test__/child", 653_000, true);
        let listing = index.list("/__sift_store_test__");
        let rows = entries_with_store_sizes(&index, listing.entries);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].size, 653_000);
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(read_entries("relative/dir").is_err());
    }
}
