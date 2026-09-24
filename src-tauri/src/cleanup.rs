//! Cross-platform deletion: items always go to the OS trash/recycle bin,
//! never permanently erased.

use serde::Serialize;
use trash::delete;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResultItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Move every path to the platform trash. Per-item errors are returned so
/// one locked file doesn't fail the whole batch.
#[tauri::command]
pub fn move_to_trash(paths: Vec<String>) -> Vec<DeleteResultItem> {
    paths
        .into_iter()
        .map(|p| match delete(&p) {
            Ok(()) => DeleteResultItem {
                path: p,
                ok: true,
                error: None,
            },
            Err(e) => DeleteResultItem {
                path: p,
                ok: false,
                error: Some(e.to_string()),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Real trash round-trip: a file we delete must vanish from its
    /// original location (the trash crate moves it to the OS trash).
    #[test]
    fn deleted_file_leaves_original_path() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sift-trash-test-{n}"));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("junk.txt");
        fs::write(&f, b"temporary").unwrap();
        assert!(f.exists());

        let results = move_to_trash(vec![f.to_string_lossy().to_string()]);
        assert_eq!(results.len(), 1);
        assert!(results[0].ok, "delete failed: {:?}", results[0].error);
        assert!(!f.exists());

        // Clean up the empty temp dir.
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_path_reports_per_item_error() {
        let results = move_to_trash(vec!["/nonexistent/sift-missing-12345".into()]);
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert!(results[0].error.is_some());
    }
}
