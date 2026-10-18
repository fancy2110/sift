//! Move entries to the operating system trash / recycle bin.
//!
//! The product's deletion contract is absolute: nothing is ever permanently
//! erased, and every failure is reported per item so one locked file cannot
//! block the rest of a batch.
//!
//! macOS uses `NSFileManager.trashItemAtURL` (silent) rather than Finder's
//! AppleScript path; see [`platform_delete`] for why.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    /// True when the batch stopped *before* this path because the caller
    /// requested cancellation. Such items were never attempted and remain on
    /// disk, unlike a failed removal.
    pub cancelled: bool,
}

impl TrashResult {
    /// A successful removal. Public so a test double or an alternative remover
    /// can report the same shape the platform remover does.
    pub fn success(path: PathBuf) -> Self {
        Self {
            path,
            ok: true,
            error: None,
            cancelled: false,
        }
    }

    /// A refused removal, with the platform's reason.
    pub fn failure(path: PathBuf, error: impl Into<String>) -> Self {
        Self {
            path,
            ok: false,
            error: Some(error.into()),
            cancelled: false,
        }
    }

    /// An item never attempted because its batch was cancelled.
    pub fn cancelled(path: PathBuf) -> Self {
        Self {
            path,
            ok: false,
            error: Some("err.cleanupCancelled".into()),
            cancelled: true,
        }
    }
}

/// A cooperative cancellation flag shared with a running trash batch.
///
/// Deletion cannot be aborted mid-system-call: once `trash::delete` handed a
/// path to Finder/Explorer, that one item is moving to the trash and the
/// result must be observed. The flag is therefore checked **between** items,
/// which is the only safe granularity. A batch of thousands of small files
/// stops within one item of the flag flipping; an individual Finder Apple
/// Event on a network volume still runs to completion.
#[derive(Clone, Default)]
pub struct TrashCancel {
    stop: Arc<AtomicBool>,
}

impl TrashCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every batch holding a clone to stop after the in-flight item.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Move every path to the platform trash, returning per-item results.
///
/// The paths are processed in order and independently: a failure on one never
/// aborts the batch. Callers must have already checked deletability policy —
/// this function does not re-check, so it is also the safety net for the
/// backend.
pub fn trash_paths(paths: &[PathBuf]) -> Vec<TrashResult> {
    trash_paths_cancellable(paths, None)
}

/// Like [`trash_paths`], but stops between items once `cancel` is set.
///
/// Paths not yet attempted come back as [`TrashResult::cancelled`]. The result
/// vector always has one entry per input path (in the original order), so a
/// position-based zip alignment cannot desync.
pub fn trash_paths_cancellable(
    paths: &[PathBuf],
    cancel: Option<&TrashCancel>,
) -> Vec<TrashResult> {
    trash_paths_impl(paths, cancel, platform_delete())
}

/// The per-item remover for this platform.
///
/// macOS: use `NSFileManager.trashItemAtURL` instead of the `trash` crate's
/// default Finder/AppleScript path. Finder plays the "move to Trash" sound
/// **once per item** and offers no per-call way to silence it, so a batch of
/// N files produced a rapid burst of N sounds. `trashItemAtURL` is silent,
/// faster (no AppleScript round trip per file), and needs no Automation
/// permission at all — the first deletion no longer prompts "Sift wants to
/// control Finder". Trade-off: Finder's "Put Back" entry may be missing for
/// items moved this way on some macOS versions (a Finder limitation outside
/// our control); the item is still in the Trash and can be dragged out to
/// restore, so the product's recoverability promise is intact.
///
/// Other platforms keep the crate default (Recycle Bin / FreeDesktop trash).
fn platform_delete() -> impl Fn(&Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::new();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        move |path: &Path| ctx.delete(path).map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        |path: &Path| trash::delete(path).map_err(|err| err.to_string())
    }
}

/// Core batch loop with the per-item removal injected, so the cancellation
/// contract (order, one result per input, stop-between-items) can be tested
/// without driving the platform trash (which needs Finder/Explorer access).
fn trash_paths_impl<F>(
    paths: &[PathBuf],
    cancel: Option<&TrashCancel>,
    delete_one: F,
) -> Vec<TrashResult>
where
    F: Fn(&Path) -> Result<(), String>,
{
    paths
        .iter()
        .map(|path| {
            if cancel.is_some_and(TrashCancel::is_cancelled) {
                return TrashResult::cancelled(path.clone());
            }
            match delete_one(path) {
                Ok(()) => TrashResult::success(path.clone()),
                Err(err) => TrashResult::failure(path.clone(), err),
            }
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
        assert!(!results[0].cancelled);
    }

    #[test]
    fn cancellation_leaves_every_remaining_path_untouched() {
        // An injected deleter records attempts but does not touch the real
        // platform trash (which needs Finder/Explorer permission).
        let attempted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempted2 = attempted.clone();
        let fake_delete = move |path: &Path| {
            attempted2.lock().unwrap().push(path.to_path_buf());
            Ok(())
        };
        let paths = vec![PathBuf::from("/tmp/sift-a"), PathBuf::from("/tmp/sift-b")];

        let cancel = TrashCancel::new();
        cancel.cancel();
        let results = trash_paths_impl(&paths, Some(&cancel), fake_delete);

        assert_eq!(results.len(), 2, "one result per input, in order");
        for result in &results {
            assert!(!result.ok);
            assert!(result.cancelled);
        }
        assert!(
            attempted.lock().unwrap().is_empty(),
            "no item may be attempted after cancellation"
        );
    }

    #[test]
    fn cancellation_after_some_items_keeps_order_and_shape() {
        // First item succeeds; the fake deleter then flips cancel. The flag is
        // polled before every following item, so both remaining items come back
        // un-attempted in their original positions.
        let paths = vec![
            PathBuf::from("/tmp/sift-ok"),
            PathBuf::from("/tmp/sift-b"),
            PathBuf::from("/tmp/sift-c"),
        ];
        let cancel = TrashCancel::new();
        let cancel2 = cancel.clone();
        let attempted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempted2 = attempted.clone();
        let fake_delete = move |path: &Path| {
            attempted2.lock().unwrap().push(path.to_path_buf());
            if path.ends_with("sift-ok") {
                cancel2.cancel();
            }
            Ok(())
        };
        let results = trash_paths_impl(&paths, Some(&cancel), fake_delete);
        assert_eq!(results.len(), 3);
        assert!(results[0].ok);
        assert!(!results[0].cancelled);
        assert!(!results[1].ok && results[1].cancelled);
        assert!(!results[2].ok && results[2].cancelled);
        assert_eq!(
            *attempted.lock().unwrap(),
            vec![PathBuf::from("/tmp/sift-ok")],
            "only the first item is attempted"
        );
    }

    #[test]
    fn a_per_item_failure_is_not_a_cancellation() {
        // Without a cancel token, the second item's failure is reported normally
        // and processing continues to the third.
        let paths = vec![
            PathBuf::from("/tmp/sift-ok"),
            PathBuf::from("/tmp/sift-missing"),
            PathBuf::from("/tmp/sift-rest"),
        ];
        let fake_delete = |path: &Path| {
            if path.ends_with("sift-missing") {
                Err("not found".to_string())
            } else {
                Ok(())
            }
        };
        let results = trash_paths_impl(&paths, None, fake_delete);
        assert!(results[0].ok);
        assert!(!results[1].ok);
        assert!(
            !results[1].cancelled,
            "a failure is distinct from cancellation"
        );
        assert!(results[2].ok, "a failure never aborts the rest of the batch");
    }

    #[test]
    fn without_a_cancel_token_every_item_is_attempted() {
        let paths = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];
        let results = trash_paths_impl(&paths, None, |_| Ok(()));
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.ok && !result.cancelled));
    }

    #[test]
    #[ignore = "requires writing to the real ~/.Trash (run outside a restricted sandbox)"]
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
    #[ignore = "requires writing to the real ~/.Trash (run outside a restricted sandbox)"]
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
