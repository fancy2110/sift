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
use sift_core::{ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest, ScanTree};
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressInfo {
    pub files: u64,
    pub dirs: u64,
    pub tracked_nodes: usize,
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
) -> Result<(), String> {
    if manager.is_running() {
        return Err("scan already running".into());
    }

    let root_path = PathBuf::from(&root);
    let engine = ScanEngine::new();
    let request = ScanRequest::new(manager.next_scan_id(), root_path.clone())
        .with_focus(PathBuf::from(&focus))
        .with_policy(ScanPolicy::thorough());

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
            pump_events(app_for_pump, handle, root_path);
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

// ---- event pump ------------------------------------------------------------

/// Drain engine events until the scan ends, re-emitting the legacy vocabulary.
fn pump_events(app: AppHandle, handle: ScanHandle, root: PathBuf) {
    let root_text = root.to_string_lossy().to_string();
    let root_key = NodeKey::from_path(&root);

    // The front end treats the first node with no parent as the tree root, so
    // publish it before anything else.
    let _ = app.emit(
        "scan://discovered",
        NodeInfo {
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
        },
    );

    let mut last_progress = Instant::now();
    let mut outcome = ScanOutcome::Failed;
    let mut files = 0u64;
    let mut dirs = 0u64;

    while let Ok(event) = handle.events.recv() {
        match event {
            ScanEvent::DirectoryListed { dir, .. } => {
                // The directory itself is known and still filling in.
                emit_sized(&app, &dir.key.to_string(), dir.size.dominant(), true);
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
                            size: entry.size.dominant(),
                            modified_ms: Some(entry.mtime_ms).filter(|value| *value != 0),
                            deletable: entry.deletable,
                            pending: entry.pending,
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
                files = progress.files;
                dirs = progress.dirs;
                if last_progress.elapsed().as_millis() >= PROGRESS_INTERVAL_MS {
                    emit_progress(&app, &handle, files, dirs);
                    last_progress = Instant::now();
                }
            }
            ScanEvent::Warning { message, .. } => {
                eprintln!("sift scan warning: {message}");
            }
            ScanEvent::Finished {
                outcome: finished, ..
            } => {
                outcome = finished;
                break;
            }
            // `ScanEvent` is non-exhaustive; an unknown future variant is
            // ignored rather than mistaken for completion.
            _ => {}
        }
    }

    emit_progress(&app, &handle, files, dirs);
    let _ = app.emit(
        "scan://done",
        serde_json::json!({
            "cancelled": matches!(outcome, ScanOutcome::Cancelled),
            "root": root_text,
        }),
    );
}

fn emit_sized(app: &AppHandle, id: &str, size: u64, pending: bool) {
    let _ = app.emit(
        "scan://sized",
        serde_json::json!({ "id": id, "size": size, "pending": pending }),
    );
}

fn emit_progress(app: &AppHandle, handle: &ScanHandle, files: u64, dirs: u64) {
    let tracked = handle
        .tree
        .lock()
        .map(|tree| tree.node_count() as usize)
        .unwrap_or(0);
    let _ = app.emit(
        "scan://progress",
        ProgressInfo {
            files,
            dirs,
            tracked_nodes: tracked,
        },
    );
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
