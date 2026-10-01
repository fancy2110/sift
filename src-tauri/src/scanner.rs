//! Tauri adapter for the shared scan engine.
//!
//! This module contains no scanning logic. It starts a [`sift_scan::ScanEngine`]
//! scan, drains the engine's events, and re-emits them through the Svelte
//! front end's batched protocol (`scan://batch`, `scan://done`).
//!
//! The scanner, the tree, the accounting and the budgets live in
//! `sift-scan` / `sift-core`; this file only knows how to be a Tauri command.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use sift_core::id::NodeKey;
use sift_core::{ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest, ScanTree};
use sift_scan::{ScanControl, ScanEngine, ScanHandle};
use sift_store::{SqliteScanJournal, StorePaths};
use tauri::{AppHandle, Emitter, Manager, State};

/// Interim progress cadence, matching what the front end animates against.
const PROGRESS_INTERVAL_MS: u128 = 250;

// ---- DTOs the Svelte front end consumes ------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub deletable: bool,
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressInfo {
    pub files: u64,
    pub dirs: u64,
    pub tracked_nodes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizedInfo {
    pub id: String,
    pub size: u64,
    pub pending: bool,
}

/// One batched scan update. The pump thread coalesces a time window of
/// engine events into a single emission so millions of files never mean
/// millions of IPC round-trips.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchUpdate {
    pub discovered: Vec<NodeInfo>,
    pub sized: Vec<SizedInfo>,
    /// Attached only on the progress cadence.
    pub progress: Option<ProgressInfo>,
}

// ---- manager state ---------------------------------------------------------

/// Owns the control handles of the running scan, plus the tree of the last one
/// so analysis commands have something to work on after the scan ends.
#[derive(Default)]
pub struct ScanManager {
    running: AtomicBool,
    paused: AtomicBool,
    /// Latest file count reported by the running scan, for the tray tooltip.
    files: AtomicU64,
    /// Whether a tray-menu refresh thread is currently running.
    menu_refreshing: AtomicBool,
    pub control: Mutex<Option<ScanControl>>,
    last_tree: Mutex<Option<Arc<Mutex<ScanTree>>>>,
    last_root: Mutex<Option<PathBuf>>,
    sequence: Mutex<u64>,
    journal: Mutex<Option<Arc<SqliteScanJournal>>>,
}

impl ScanManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Whether the running scan is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Latest file count reported by the running scan.
    pub fn file_count(&self) -> u64 {
        self.files.load(Ordering::SeqCst)
    }

    /// Remember a progress file count from the event pump.
    pub fn set_file_count(&self, files: u64) {
        self.files.store(files, Ordering::SeqCst);
    }

    /// Pause the scan running on this manager.
    pub fn pause_current(&self) {
        if let Some(control) = self.control.lock().unwrap().as_ref() {
            control.pause();
        }
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume the scan running on this manager.
    pub fn resume_current(&self) {
        if let Some(control) = self.control.lock().unwrap().as_ref() {
            control.resume();
        }
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Cancel the running scan.
    pub fn cancel_current(&self) {
        if let Some(control) = self.control.lock().unwrap().as_ref() {
            control.cancel();
        }
    }

    /// Atomically claim the tray-menu refresh role; false if another thread
    /// already holds it.
    pub fn try_start_menu_refresh(&self) -> bool {
        self.menu_refreshing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Release the tray-menu refresh role.
    pub fn finish_menu_refresh(&self) {
        self.menu_refreshing.store(false, Ordering::SeqCst);
    }

    /// The tree produced by the most recent scan, if any.
    pub fn last_tree(&self) -> Option<Arc<Mutex<ScanTree>>> {
        self.last_tree.lock().unwrap().clone()
    }

    pub fn last_root(&self) -> Option<PathBuf> {
        self.last_root.lock().unwrap().clone()
    }

    /// The durable scan journal, opened on first use. A failure to open it
    /// yields `None`, and the caller runs a plain (non-resumable) scan.
    fn journal(&self) -> Option<Arc<SqliteScanJournal>> {
        let mut guard = self.journal.lock().unwrap();
        if let Some(journal) = guard.as_ref() {
            return Some(Arc::clone(journal));
        }
        let path = StorePaths::discover()?;
        let journal = SqliteScanJournal::open(path.scan_journal()).ok()?;
        let journal = Arc::new(journal);
        *guard = Some(Arc::clone(&journal));
        Some(journal)
    }

    fn next_scan_id(&self) -> ScanId {
        let mut sequence = self.sequence.lock().unwrap();
        *sequence += 1;
        ScanId(*sequence)
    }
}

// ---- commands --------------------------------------------------------------

/// Start (or restart) a scan. Returns as soon as the engine is running; results
/// arrive as events.
#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    manager: State<'_, ScanManager>,
    root: String,
    focus: String,
) -> Result<(), String> {
    start_scan_with(&app, &manager, root, focus)
}

/// Start a scan against a concrete manager, callable from non-command code.
pub fn start_scan_with(
    app: &AppHandle,
    manager: &ScanManager,
    root: String,
    focus: String,
) -> Result<(), String> {
    if manager.is_running() {
        return Err("scan already running".into());
    }

    manager.paused.store(false, Ordering::SeqCst);
    manager.files.store(0, Ordering::SeqCst);
    let root_path = PathBuf::from(&root);
    let engine = ScanEngine::new();
    let request = ScanRequest::new(manager.next_scan_id(), root_path.clone())
        .with_focus(PathBuf::from(&focus))
        .with_policy(ScanPolicy::thorough());

    let handle = match manager.journal() {
        Some(journal) => engine
            .scan_with_journal(request, journal as Arc<dyn sift_scan::ScanJournal>)
            .map_err(|err| err.to_string())?,
        None => engine.scan(request).map_err(|err| err.to_string())?,
    };
    let control = handle.control();

    {
        let mut slot = manager.control.lock().unwrap();
        *slot = Some(control);
    }
    {
        let mut slot = manager.last_tree.lock().unwrap();
        *slot = Some(Arc::clone(&handle.tree));
    }
    {
        let mut slot = manager.last_root.lock().unwrap();
        *slot = Some(root_path.clone());
    }
    manager.running.store(true, Ordering::SeqCst);

    let app_for_pump = AppHandle::clone(app);
    std::thread::Builder::new()
        .name("sift-tauri-scan".into())
        .spawn(move || {
            pump_events(app_for_pump.clone(), handle, root_path);
            // The engine finished; clear the running flag through the app state.
            if let Some(manager) = app_for_pump.try_state::<ScanManager>() {
                manager.running.store(false, Ordering::SeqCst);
                manager.paused.store(false, Ordering::SeqCst);
                manager.menu_refreshing.store(false, Ordering::SeqCst);
                *manager.control.lock().unwrap() = None;
            }
        })
        .map_err(|err| err.to_string())?;

    Ok(())
}

/// Re-prioritize the running scan toward `focus`.
#[tauri::command]
pub fn set_scan_focus(manager: State<'_, ScanManager>, focus: String) {
    if let Some(control) = manager.control.lock().unwrap().as_ref() {
        control.set_focus(PathBuf::from(focus));
    }
}

/// Cancel the running scan; a `scan://done` with `cancelled: true` follows.
#[tauri::command]
pub fn cancel_scan(manager: State<'_, ScanManager>) {
    if let Some(control) = manager.control.lock().unwrap().as_ref() {
        control.cancel();
    }
}

/// Pause the running scan. In-flight directory reads finish, then discovery
/// stops until [`resume_scan`].
#[tauri::command]
pub fn pause_scan(manager: State<'_, ScanManager>) {
    if let Some(control) = manager.control.lock().unwrap().as_ref() {
        control.pause();
    }
}

/// Resume a paused scan.
#[tauri::command]
pub fn resume_scan(manager: State<'_, ScanManager>) {
    if let Some(control) = manager.control.lock().unwrap().as_ref() {
        control.resume();
    }
}

/// Whether a scan is currently running.
#[tauri::command]
pub fn scan_running(manager: State<'_, ScanManager>) -> bool {
    manager.is_running()
}

// ---- event pump ------------------------------------------------------------

/// Drain engine events until the scan ends, re-emitting the legacy vocabulary.
fn pump_events(app: AppHandle, handle: ScanHandle, root: PathBuf) {
    let root_text = root.to_string_lossy().to_string();
    let root_key = NodeKey::from_path(&root);

    // The front end treats the first node with no parent as the tree root. It
    // goes out in the first batch, before any engine events.
    let root_info = NodeInfo {
        id: root_key.to_string(),
        parent_id: None,
        name: root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root_text.clone()),
        path: root_text.clone(),
        is_dir: true,
        size: 0,
        modified_ms: None,
        deletable: false,
        pending: true,
    };

    let mut last_progress = Instant::now();
    let mut outcome = ScanOutcome::Failed;
    let mut files = 0u64;
    let mut dirs = 0u64;

    let mut discovered: Vec<NodeInfo> = vec![root_info];
    let mut sized: Vec<SizedInfo> = Vec::new();
    let mut progress_due = false;

    let flush = |discovered: &mut Vec<NodeInfo>,
                 sized: &mut Vec<SizedInfo>,
                 progress: Option<ProgressInfo>| {
        if discovered.is_empty() && sized.is_empty() && progress.is_none() {
            return;
        }
        let _ = app.emit(
            "scan://batch",
            BatchUpdate {
                discovered: std::mem::take(discovered),
                sized: std::mem::take(sized),
                progress,
            },
        );
    };

    // Coalesce every engine event arriving inside BATCH_WINDOW_MS into one
    // emission; an idle receiver flushes early so the visible directory never
    // waits for the window to fill.
    const BATCH_WINDOW_MS: u64 = 64;
    'pump: loop {
        let event = match handle
            .events
            .recv_timeout(std::time::Duration::from_millis(BATCH_WINDOW_MS))
        {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                let progress = if progress_due {
                    Some(build_progress(&handle, files, dirs))
                } else {
                    None
                };
                progress_due = false;
                flush(&mut discovered, &mut sized, progress);
                continue 'pump;
            }
            Err(RecvTimeoutError::Disconnected) => break 'pump,
        };
        match event {
            ScanEvent::NodesAdopted { nodes, .. } => {
                for node in nodes {
                    discovered.push(NodeInfo {
                        id: node.key,
                        parent_id: if node.parent_key.is_empty() {
                            None
                        } else {
                            Some(node.parent_key)
                        },
                        name: node.name,
                        path: node.path.clone(),
                        is_dir: true,
                        size: node.size,
                        modified_ms: Some(node.mtime_ms).filter(|value| *value != 0),
                        deletable: node.deletable,
                        pending: !node.closed,
                    });
                }
            }
            ScanEvent::DirectoryListed { dir, .. } => {
                // The directory itself is known and still filling in.
                sized.push(SizedInfo {
                    id: dir.key.to_string(),
                    size: dir.size.dominant(),
                    pending: true,
                });
                for entry in &dir.entries {
                    let path = join_path(&dir.path, &entry.name);
                    discovered.push(NodeInfo {
                        id: entry.key.to_string(),
                        parent_id: Some(dir.key.to_string()),
                        name: entry.name.clone(),
                        path,
                        is_dir: entry.is_dir,
                        size: entry.size.dominant(),
                        modified_ms: Some(entry.mtime_ms).filter(|value| *value != 0),
                        deletable: entry.deletable,
                        pending: entry.pending,
                    });
                }
            }
            ScanEvent::DirectorySized {
                key,
                size: new_size,
                pending,
                ..
            } => {
                sized.push(SizedInfo {
                    id: key.to_string(),
                    size: new_size.dominant(),
                    pending,
                });
            }
            ScanEvent::DirectoryClosed { .. } => {
                // The final size arrives through the `DirectorySized` flush;
                // nothing extra to publish here.
            }
            ScanEvent::Progress { progress, .. } => {
                files = progress.files;
                dirs = progress.dirs;
                if let Some(manager) = app.try_state::<ScanManager>() {
                    manager.set_file_count(files);
                }
                if last_progress.elapsed().as_millis() >= PROGRESS_INTERVAL_MS {
                    progress_due = true;
                    last_progress = Instant::now();
                }
            }
            ScanEvent::Warning { message, .. } => {
                eprintln!("sift scan warning: {message}");
            }
            ScanEvent::Calibrated { key, size, .. } => {
                // Exact size of a previously predicted giant directory.
                sized.push(SizedInfo {
                    id: key.to_string(),
                    size: size.dominant(),
                    pending: false,
                });
            }
            ScanEvent::CalibrationFinished { .. } => {
                break;
            }
            ScanEvent::Finished {
                outcome: finished, ..
            } => {
                // The main scan is done; keep pumping only when background
                // calibration follows. With nothing to calibrate the event
                // sender is dropped and the next receive disconnects.
                outcome = finished;
            }
            // `ScanEvent` is non-exhaustive; an unknown future variant is
            // ignored rather than mistaken for completion.
            _ => {}
        }
    }

    // Drain anything still buffered, always with a final progress reading.
    flush(
        &mut discovered,
        &mut sized,
        Some(build_progress(&handle, files, dirs)),
    );
    let _ = app.emit(
        "scan://done",
        serde_json::json!({
            "cancelled": matches!(outcome, ScanOutcome::Cancelled),
            "root": root_text,
        }),
    );
}

fn build_progress(handle: &ScanHandle, files: u64, dirs: u64) -> ProgressInfo {
    let tracked = handle
        .tree
        .lock()
        .map(|tree| tree.node_count() as usize)
        .unwrap_or(0);
    ProgressInfo {
        files,
        dirs,
        tracked_nodes: tracked,
    }
}

/// Join a directory path with an entry name, tolerating a trailing separator.
fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with(std::path::MAIN_SEPARATOR) {
        format!("{dir}{name}")
    } else {
        format!("{dir}{}{name}", std::path::MAIN_SEPARATOR)
    }
}

/// Compatibility shim for the old `node_id` helper used by the watcher module.
pub fn node_id(path: &str) -> String {
    NodeKey::from_path(Path::new(path)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_are_stable_and_path_derived() {
        assert_eq!(node_id("/a/b"), node_id("/a/b"));
        assert_ne!(node_id("/a/b"), node_id("/a/c"));
        assert_eq!(
            NodeKey::from_hex(&node_id("/a/b")).unwrap(),
            NodeKey::from_path(Path::new("/a/b"))
        );
    }

    #[test]
    fn joining_paths_handles_both_separator_cases() {
        assert_eq!(join_path("/Users/me", "Downloads"), "/Users/me/Downloads");
        assert_eq!(join_path("/", "Users"), "/Users");
        // A root that already ends in a separator must not double it.
        let with_sep = format!("/tmp{}", std::path::MAIN_SEPARATOR);
        assert_eq!(join_path(&with_sep, "x"), "/tmp/x");
    }

    #[test]
    fn the_manager_starts_idle() {
        let manager = ScanManager::new();
        assert!(!manager.is_running());
        assert!(manager.last_tree().is_none());
        assert!(manager.last_root().is_none());
        assert_eq!(manager.next_scan_id(), ScanId(1));
        assert_eq!(manager.next_scan_id(), ScanId(2));
    }
}
