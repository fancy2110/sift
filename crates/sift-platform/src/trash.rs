//! Move entries to the operating system trash / recycle bin.
//!
//! The product's deletion contract is absolute: nothing is ever permanently
//! erased, and every failure is reported per item so one locked file cannot
//! block the rest of a batch.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The outcome of trashing one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TrashResult {
    pub path: PathBuf,
    pub ok: bool,
    pub error: Option<String>,
}

impl TrashResult {
    /// A successful removal. Public so a test double or an alternative remover
    /// can report the same shape the platform remover does.
    pub fn success(path: PathBuf) -> Self {
        Self {
            path,
            ok: true,
            error: None,
        }
    }

    /// A refused removal, with the platform's reason.
    pub fn failure(path: PathBuf, error: impl Into<String>) -> Self {
        Self {
            path,
            ok: false,
            error: Some(error.into()),
        }
    }
}

/// Move every path to the platform trash, returning per-item results.
///
/// The paths are processed in order and independently: a failure on one never
/// aborts the batch. Callers must have already checked deletability policy —
/// this function does not re-check, so it is also the safety net for the
/// backend.
pub fn trash_paths(paths: &[PathBuf]) -> Vec<TrashResult> {
    paths
        .iter()
        .map(|path| match trash::delete(path) {
            Ok(()) => TrashResult::success(path.clone()),
            Err(err) => TrashResult::failure(path.clone(), err.to_string()),
        })
        .collect()
}

/// Whether the platform trash is available at all (e.g. not on a read-only
/// root). Kept separate so a UI can disable the whole action instead of
/// failing per item.
pub fn trash_available() -> bool {
    // `trash` resolves a trash dir lazily per call; the cheapest truthful probe
    // is trashing nothing and seeing whether resolution itself fails.
    trash_paths(&[]).is_empty()
}

/// Move one path, returning the single result.
pub fn trash_path(path: &Path) -> TrashResult {
    trash_paths(std::slice::from_ref(&path.to_path_buf())).remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_path_reports_per_item_error() {
        let results = trash_paths(&[PathBuf::from("/nonexistent/sift-missing-12345")]);
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert!(results[0].error.is_some());
    }

    #[test]
    #[ignore = "requires macOS Finder Automation (System Settings → Privacy → Automation) permission"]
    fn real_file_round_trips_through_the_trash() {
        let base = std::env::temp_dir().join(format!("sift-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let file = base.join("junk.txt");
        fs::write(&file, b"temporary").unwrap();
        assert!(file.exists());

        let result = trash_path(&file);
        assert!(result.ok, "trash failed: {:?}", result.error);
        assert!(!file.exists(), "file must leave its original location");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    #[ignore = "requires macOS Finder Automation permission"]
    fn batch_preserves_order_and_independence() {
        let base = std::env::temp_dir().join(format!("sift-trash-batch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let a = base.join("a.txt");
        fs::write(&a, b"a").unwrap();

        let results = trash_paths(&[a.clone(), PathBuf::from("/nonexistent/x")]);
        assert!(results[0].ok);
        assert!(!results[1].ok);
        assert_eq!(results[0].path, a);
        assert!(!a.exists());

        let _ = fs::remove_dir_all(&base);
    }
}
