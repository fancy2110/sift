//! Tauri adapter for deletion.
//!
//! The contract is unchanged from the original implementation and is worth
//! restating: items always go to the OS trash, never permanently erased, and
//! failures are reported per item so one locked file cannot fail a batch.

use serde::Serialize;
use tauri::command;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResultItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Move every path to the platform trash.
#[command]
pub fn move_to_trash(paths: Vec<String>) -> Vec<DeleteResultItem> {
    let as_paths: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    sift_platform::trash::trash_paths(&as_paths)
        .into_iter()
        .map(|result| DeleteResultItem {
            path: result.path.to_string_lossy().into_owned(),
            ok: result.ok,
            error: result.error,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_paths_report_a_per_item_error() {
        let results = move_to_trash(vec![
            "/nonexistent/sift-missing-a".into(),
            "/nonexistent/sift-missing-b".into(),
        ]);
        assert_eq!(results.len(), 2);
        for result in &results {
            assert!(!result.ok);
            assert!(result.error.is_some());
        }
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

        let results = move_to_trash(vec![file.to_string_lossy().into_owned()]);
        assert!(results[0].ok, "{:?}", results[0].error);
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
