//! Tauri adapter for the shared scan engine.
//!
//! This module contains no scanning logic. It starts a [`sift_scan::ScanEngine`]
//! scan, drains the engine's events, and re-emits them in the vocabulary the
//! existing Svelte front end already understands
//! (`scan://discovered`, `scan://sized`, `scan://progress`, `scan://done`).
//!
//! That translation is the whole point of the refactor: the scanner, the tree,
//! the accounting and the budgets live in `sift-scan` / `sift-core`, shared with
//! the native GPUI front end, while this file only knows how to be a Tauri
//! command.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use sift_core::id::NodeKey;
use sift_core::{Progress, ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest, ScanTree};
use sift_scan::{ScanControl, ScanEngine, ScanHandle};
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
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressInfo {
    pub files: u64,
    pub dirs: u64,
    pub tracked_nodes: usize,
    pub bytes: u64,
    pub denied: u64,
    pub queue_depth: usize,
    pub elapsed_secs: f64,
    pub entries_per_sec: u64,
    pub bytes_per_sec: u64,
    pub coverage: f32,
}

impl From<&Progress> for ProgressInfo {
    fn from(value: &Progress) -> Self {
        Self {
            files: value.files,
            dirs: value.dirs,
            tracked_nodes: 0,
            // Report the bytes the volume actually spends, not the logical
            // file size the files claim.
            bytes: value.bytes.reclaimable(),
            denied: value.denied,
            queue_depth: value.queue_depth as usize,
            elapsed_secs: value.elapsed_secs,
            entries_per_sec: value.entries_per_sec() as u64,
            bytes_per_sec: value.bytes_per_sec() as u64,
            coverage: value.coverage as f32,
        }
    }
}

// ---- manager state ---------------------------------------------------------

/// Owns the control handles of the running scan, plus the tree of the last one
/// so analysis commands have something to work on after the scan ends.
#[derive(Default)]
pub struct ScanManager {
    running: AtomicBool,
    control: Mutex<Option<ScanControl>>,
    last_tree: Mutex<Option<Arc<Mutex<ScanTree>>>>,
    last_root: Mutex<Option<PathBuf>>,
    sequence: Mutex<u64>,
}

impl ScanManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// The tree produced by the most recent scan, if any.
    pub fn last_tree(&self) -> Option<Arc<Mutex<ScanTree>>> {
        self.last_tree.lock().unwrap().clone()
    }

    pub fn last_root(&self) -> Option<PathBuf> {
        self.last_root.lock().unwrap().clone()
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
    label: String,
) -> Result<(), String> {
    if manager.is_running() {
        return Err("scan already running".into());
    }

    let root_path = PathBuf::from(&root);

    // One snapshot database per scanned root, kept in the app's data dir.
    let engine = ScanEngine::new();
    let engine = attach_snapshots(engine, &app, &root_path);

    let request = ScanRequest::new(manager.next_scan_id(), root_path.clone())
        .with_focus(PathBuf::from(&focus))
        .with_policy(ScanPolicy::thorough());

    // Feed the volume's in-use byte count so the progress bar can show real
    // coverage instead of staying at 0%.
    let request = match sift_platform::volume::free_space(&root_path) {
        Some((total, available)) => request.with_volume_used_bytes(total.saturating_sub(available)),
        None => request,
    };

    let handle = engine.scan(request).map_err(|err| err.to_string())?;
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

    let app_for_pump = app.clone();
    std::thread::Builder::new()
        .name("sift-tauri-scan".into())
        .spawn(move || {
            pump_events(app_for_pump, handle, root_path, label);
            // The engine finished; clear the running flag through the app state.
            if let Some(manager) = app.try_state::<ScanManager>() {
                manager.running.store(false, Ordering::SeqCst);
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

/// Whether a scan is currently running.
#[tauri::command]
pub fn scan_running(manager: State<'_, ScanManager>) -> bool {
    manager.is_running()
}

/// Attach the per-root SQLite snapshot store, logging instead of failing when
/// the data directory is unavailable (snapshots are an optimisation).
fn attach_snapshots(engine: ScanEngine, app: &AppHandle, root: &Path) -> ScanEngine {
    let Some(data_dir) = app.path().app_data_dir().ok() else {
        return engine;
    };
    let root_key = sift_core::id::NodeKey::from_path(root).to_string();
    let db_path = data_dir.join("snapshots").join(format!("{root_key}.db"));
    match sift_persist::SqliteSnapshotStore::open(db_path) {
        Ok(store) => engine.with_snapshot_store(Arc::new(store)),
        Err(err) => {
            eprintln!("sift snapshots disabled for {}: {err}", root.display());
            engine
        }
    }
}

// ---- event pump ------------------------------------------------------------

/// Drain engine events until the scan ends, re-emitting the legacy vocabulary.
fn pump_events(app: AppHandle, handle: ScanHandle, root: PathBuf, root_label: String) {
    let root_text = root.to_string_lossy().to_string();
    let root_key = NodeKey::from_path(&root);

    // The front end treats the first node with no parent as the tree root, so
    // publish it before anything else.
    let _ = app.emit(
        "scan://discovered",
        NodeInfo {
            id: root_key.to_string(),
            parent_id: None,
            name: root_label,
            path: root_text.clone(),
            is_dir: true,
            size: 0,
            modified_ms: None,
            deletable: false,
            pending: true,
            estimated: false,
        },
    );

    let mut last_progress = Instant::now();
    let mut outcome = ScanOutcome::Failed;
    let mut latest_progress: Option<Progress> = None;
    let mut scan_finished = false;

    while let Ok(event) = handle.events.recv() {
        match event {
            ScanEvent::DirectoryListed { dir, .. } => {
                // The directory itself is known and still filling in.
                emit_sized(&app, &dir.key.to_string(), dir.size.reclaimable(), true);
                for entry in &dir.entries {
                    let path = join_path(&dir.path, &entry.name);
                    let _ = app.emit(
                        "scan://discovered",
                        NodeInfo {
                            id: entry.key.to_string(),
                            parent_id: Some(dir.key.to_string()),
                            name: entry.name.clone(),
                            path,
                            is_dir: entry.is_dir,
                            size: entry.size.reclaimable(),
                            modified_ms: Some(entry.mtime_ms).filter(|value| *value != 0),
                            deletable: entry.deletable,
                            pending: entry.pending,
                            estimated: false,
                        },
                    );
                }
            }
            ScanEvent::DirectorySized {
                key,
                size,
                pending,
                ..
            } => {
                emit_sized(&app, &key.to_string(), size.dominant(), pending);
            }
            ScanEvent::DirectoryClosed { .. } => {
                // The final size arrives through the `DirectorySized` flush;
                // nothing extra to publish here.
            }
            ScanEvent::Progress { progress, .. } => {
                if last_progress.elapsed().as_millis() >= PROGRESS_INTERVAL_MS {
                    emit_progress(&app, &progress);
                    last_progress = Instant::now();
                }
                latest_progress = Some(progress);
            }
            ScanEvent::Warning { message, .. } => {
                eprintln!("sift scan warning: {message}");
                if message.contains("校准") {
                    let _ = app.emit(
                        "scan://calibration-start",
                        serde_json::json!({ "message": message }),
                    );
                }
            }
            ScanEvent::Calibrated { key, size, files, .. } => {
                // The main scan is done; a predicted giant subtree was just
                // measured. Forward it so the UI can patch the totals.
                let _ = app.emit(
                    "scan://calibrated",
                    serde_json::json!({
                        "id": key.to_string(),
                        "size": size.reclaimable(),
                        "files": files,
                    }),
                );
            }
            ScanEvent::CalibrationFinished { .. } => {
                let _ = app.emit("scan://calibration-done", serde_json::json!({}));
                if scan_finished {
                    break;
                }
            }
            ScanEvent::Finished {
                outcome: finished, ..
            } => {
                outcome = finished;
                scan_finished = true;
                // Emit done now so the UI becomes interactive, then keep
                // draining background calibration events.
                if let Some(progress) = latest_progress.take() {
                    emit_progress(&app, &progress);
                }
                let _ = app.emit(
                    "scan://done",
                    serde_json::json!({
                        "cancelled": matches!(outcome, ScanOutcome::Cancelled),
                        "root": root_text,
                    }),
                );
            }
            // `ScanEvent` is non-exhaustive; an unknown future variant is
            // ignored rather than mistaken for completion.
            _ => {}
        }
    }

    // Fallback if the channel closed before the terminal marker arrived.
    if !scan_finished {
        if let Some(progress) = latest_progress {
            emit_progress(&app, &progress);
        }
        let _ = app.emit(
            "scan://done",
            serde_json::json!({
                "cancelled": matches!(outcome, ScanOutcome::Cancelled),
                "root": root_text,
            }),
        );
    }
}

fn emit_sized(app: &AppHandle, id: &str, size: u64, pending: bool) {
    let _ = app.emit(
        "scan://sized",
        serde_json::json!({ "id": id, "size": size, "pending": pending }),
    );
}

fn emit_progress(app: &AppHandle, progress: &Progress) {
    let _ = app.emit("scan://progress", ProgressInfo::from(progress));
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
        assert_eq!(NodeKey::from_hex(&node_id("/a/b")).unwrap(), NodeKey::from_path(Path::new("/a/b")));
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
