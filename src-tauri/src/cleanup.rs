//! Tauri adapter for deletion.
//!
//! This milestone is dry-run by default: the preview command checks existence
// and returns exactly what *would* happen without touching the filesystem.
//! The real trash call is kept behind an explicit `execute` flag so no code
//! path deletes unless the caller opts in.

use serde::Serialize;
use tauri::command;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResultItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
    /// Whether this result was produced without an actual move.
    pub dry_run: bool,
}

/// Describe what deleting `paths` would do, without doing it.
///
/// Every existing path reports `ok: true, dry_run: true`; a missing path
/// reports a per-item error. Nothing is moved, trashed, or unlinked.
#[command]
pub fn preview_delete(paths: Vec<String>) -> Vec<DeleteResultItem> {
    paths
        .into_iter()
        .map(|path| {
            let exists = std::path::Path::new(&path).exists();
            DeleteResultItem {
                path,
                ok: exists,
                error: if exists {
                    None
                } else {
                    Some("path not found; nothing would be removed.".into())
                },
                dry_run: true,
            }
        })
        .collect()
}

/// Move every path to the platform trash — only when `execute` is explicitly
/// `true`. With `false` (the safe default) it behaves exactly like
/// [`preview_delete`].
#[command]
pub fn move_to_trash(paths: Vec<String>, execute: bool) -> Vec<DeleteResultItem> {
    if !execute {
        return preview_delete(paths);
    }
    let as_paths: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    sift_platform::trash::trash_paths(&as_paths)
        .into_iter()
        .map(|result| DeleteResultItem {
            path: result.path.to_string_lossy().into_owned(),
            ok: result.ok,
            error: result.error,
            dry_run: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_reports_existing_paths_without_touching_them() {
        let file = std::env::temp_dir().join(format!("sift-dryrun-{}", std::process::id()));
        std::fs::write(&file, b"keep").unwrap();
        let results = preview_delete(vec![
            file.to_string_lossy().into_owned(),
            "/nonexistent/sift-missing".into(),
        ]);
        assert_eq!(results.len(), 2);
        assert!(results[0].ok && results[0].dry_run);
        assert!(!results[1].ok && results[1].dry_run);
        // The file must still exist: dry-run never deletes.
        assert!(file.exists());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn move_to_trash_without_execute_is_a_dry_run() {
        let results = move_to_trash(vec!["/whatever".into()], false);
        assert!(results[0].dry_run);
    }

    /// The real trash round-trip needs macOS Finder Automation permission, so it
    /// runs only when explicitly requested.
    #[test]
    #[ignore = "requires macOS Finder Automation permission"]
    fn a_real_file_leaves_its_original_path() {
        let dir = std::env::temp_dir().join(format!("sift-tauri-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("junk.txt");
        std::fs::write(&file, b"temporary").unwrap();

        let results = move_to_trash(vec![file.to_string_lossy().into_owned()], true);
        assert!(results[0].ok, "{:?}", results[0].error);
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
