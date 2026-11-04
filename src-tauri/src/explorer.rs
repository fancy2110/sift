//! On-demand directory browsing, independent of a scan.
//!
//! The Explorer must work before (or without) a full scan: it lists one
//! directory at a time and computes child-folder totals lazily. Sizes are
//! bounded per directory so a huge folder cannot pin the UI forever.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Immediate children of one directory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// File size for files; 0 for directories until `dir_sizes` fills it.
    pub size: u64,
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
const MAX_ENTRIES_PER_WALK: u64 = 200_000;

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

#[tauri::command]
pub async fn list_dir_entries(path: String) -> Result<Vec<DirEntryDto>, String> {
    tauri::async_runtime::spawn_blocking(move || list_entries(&path))
        .await
        .map_err(|error| error.to_string())?
}

fn list_entries(path: &str) -> Result<Vec<DirEntryDto>, String> {
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
        entries.push(DirEntryDto {
            name,
            path: entry_path.to_string_lossy().into_owned(),
            is_dir,
            size,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
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

fn recursive_size(path: &str) -> DirSizeDto {
    let mut size = 0u64;
    let mut visited = 0u64;
    let mut partial = false;
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
                size = size.saturating_add(meta.len());
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
        let entries = list_entries(root.to_str().unwrap()).unwrap();
        assert_eq!(entries[0].name, "a");
        assert!(entries[0].is_dir);
        let top = entries.iter().find(|entry| entry.name == "top.txt").unwrap();
        assert_eq!(top.size, 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recursive_size_sums_without_following_links() {
        let root = fixture("size");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("b"), root.join("a/link")).unwrap();
        let total = recursive_size(root.join("a").to_str().unwrap());
        assert_eq!(total.size, 8, "5 + 3 bytes, symlink skipped");
        assert!(!total.partial);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(list_entries("relative/dir").is_err());
    }
}
