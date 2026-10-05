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
use sift_core::{
    DirCloseStatus, ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest, ScanTree,
};
use sift_scan::{ScanControl, ScanEngine, ScanHandle};
use sift_store::{SqliteScanJournal, StorePaths};
use tauri::{AppHandle, Emitter, Manager, State};

/// Interim progress cadence, matching what the front end animates against.
const PROGRESS_INTERVAL_MS: u128 = 250;

// ---- DTOs the Svelte front end consumes ------------------------------------

/// How a directory's size was obtained. Serialized as the lowercase string
/// the Svelte store renders badges from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    /// Walked and (once closed) measured exactly.
    Ok,
    /// Main scan skipped it; exact calibration is pending or in progress.
    Estimated,
    /// Unreadable or explicitly skipped by the user; size is unknown.
    Denied,
    /// Parked waiting for a macOS authorization decision.
    Awaiting,
}

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
    pub status: NodeStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressInfo {
    pub files: u64,
    pub dirs: u64,
    /// Logical bytes accounted for so far.
    pub bytes: u64,
    /// Bytes in use on the scanned volume (the percent denominator).
    pub total_bytes: u64,
    /// `0..=100`, byte-coverage based.
    pub percent: f64,
    pub rate_bytes_per_sec: f64,
    /// Directories parked pending an authorization decision.
    pub awaiting: u64,
    pub tracked_nodes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizedInfo {
    pub id: String,
    /// New size, or `None` for a status-only update that must not change the
    /// last size the UI already showed.
    pub size: Option<u64>,
    pub pending: bool,
    pub status: NodeStatus,
}

/// A directory is parked pending macOS authorization; the front end shows a
/// card with "grant" / "skip" actions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionInfo {
    pub id: String,
    pub path: String,
    pub name: String,
    /// `true` — grantable via an open panel (TCC); `false` — needs an admin.
    pub tcc: bool,
}

/// One batched scan update. The pump thread coalesces a time window of
/// engine events into a single emission so millions of files never mean
/// millions of IPC round-trips.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchUpdate {
    /// Id of the scan this batch belongs to. The front end drops batches whose
    /// epoch differs from the scan it is displaying, so late events from a
    /// cancelled/replaced scan cannot repopulate a freshly reset tree.
    pub epoch: u64,
    pub discovered: Vec<NodeInfo>,
    pub sized: Vec<SizedInfo>,
    pub permissions: Vec<PermissionInfo>,
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
/// arrive as events. The returned value is the scan's epoch id, which tags
/// every event the scan emits.
#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    manager: State<'_, ScanManager>,
    root: String,
    focus: String,
) -> Result<u64, String> {
    start_scan_with(&app, &manager, root, focus)
}

/// Start a scan against a concrete manager, callable from non-command code.
/// Returns the new scan's epoch id on success.
pub fn start_scan_with(
    app: &AppHandle,
    manager: &ScanManager,
    root: String,
    focus: String,
) -> Result<u64, String> {
    if manager.is_running() {
        return Err("scan already running".into());
    }

    // A previous scan's coordinator may be lingering so its authorization
    // cards stay usable. Stop it and take its control before installing the
    // replacement; the old pump then disconnects on its own.
    if let Some(old) = manager.control.lock().unwrap().take() {
        old.shutdown();
    }

    manager.paused.store(false, Ordering::SeqCst);
    manager.files.store(0, Ordering::SeqCst);
    let root_path = PathBuf::from(&root);

    // Bytes actually in use on the target volume: feeds the percent ring and
    // the engine's coverage. `free_space` only matches absolute paths.
    let volume_used = sift_platform::volume::free_space(&root_path)
        .map(|(total, available)| total.saturating_sub(available))
        .unwrap_or(0);

    let engine = ScanEngine::new();
    let mut request = ScanRequest::new(manager.next_scan_id(), root_path.clone())
        .with_focus(PathBuf::from(&focus))
        .with_policy(ScanPolicy::thorough());
    let epoch = request.id.0;
    if volume_used > 0 {
        request = request.with_volume_used_bytes(volume_used);
    }

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
            // Finished sets running=false; the control is kept in the manager
            // after the pump exits so post-completion grants could still have
            // been accepted, and a replacing scan clears it in
            // `start_scan_with`.
            pump_events(app_for_pump, handle, root_path, volume_used);
        })
        .map_err(|err| err.to_string())?;

    Ok(epoch)
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

/// The epoch id of the currently running scan, or `None` when idle. The front
/// end uses the id to accept events from a scan that survived a webview reload.
#[tauri::command]
pub fn scan_running(manager: State<'_, ScanManager>) -> Option<u64> {
    if !manager.is_running() {
        return None;
    }
    manager
        .control
        .lock()
        .unwrap()
        .as_ref()
        .map(|control| control.scan_id().0)
}

/// Feed the user's decision back to a directory parked pending authorization.
/// `granted = true` re-walks it; `false` closes it as denied/unknown. When the
/// user selected an ancestor folder in the panel, `granted_path` carries it.
#[tauri::command]
pub fn resolve_permission(
    manager: State<'_, ScanManager>,
    id: String,
    granted: bool,
    granted_path: Option<String>,
) -> Result<(), String> {
    let control = manager.control.lock().unwrap();
    let Some(control) = control.as_ref() else {
        return Err("no active scan".into());
    };
    control
        .resolve_permission(id, granted, granted_path)
        .map_err(|err| err.to_string())
}

// ---- event pump ------------------------------------------------------------

/// Drain engine events until the scan ends, re-emitting the legacy vocabulary.
fn pump_events(
    app: AppHandle,
    handle: ScanHandle,
    root: PathBuf,
    volume_used: u64,
) {
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
        status: NodeStatus::Ok,
    };

    let mut last_progress = Instant::now();
    let mut outcome = ScanOutcome::Failed;
    let mut latest = sift_core::Progress::default();

    let mut discovered: Vec<NodeInfo> = vec![root_info];
    let mut sized: Vec<SizedInfo> = Vec::new();
    let mut permissions: Vec<PermissionInfo> = Vec::new();
    let mut progress_due = false;

    let epoch = handle.scan_id.0;
    let flush = |discovered: &mut Vec<NodeInfo>,
                 sized: &mut Vec<SizedInfo>,
                 permissions: &mut Vec<PermissionInfo>,
                 progress: Option<ProgressInfo>,
                 last_flush: &mut Instant| {
        if discovered.is_empty()
            && sized.is_empty()
            && permissions.is_empty()
            && progress.is_none()
        {
            return;
        }
        let result = app.emit(
            "scan://batch",
            BatchUpdate {
                epoch,
                discovered: std::mem::take(discovered),
                sized: std::mem::take(sized),
                permissions: std::mem::take(permissions),
                progress,
            },
        );
        if let Err(err) = result {
            eprintln!("scan batch emit failed: {err}");
        }
        *last_flush = Instant::now();
    };

    // Events are coalesced into one emission per BATCH_WINDOW_MS of wall clock,
    // and never beyond the per-batch caps. Flushing on a deadline (rather than
    // only on an idle gap, as an earlier version did) bounds IPC payloads even
    // when the engine streams events without pause — a full-volume burst would
    // otherwise become a single multi-million-node batch that pins the webview
    // for minutes and consumes gigabytes.
    const BATCH_WINDOW_MS: u64 = 64;
    const BATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(BATCH_WINDOW_MS);
    const MAX_DISCOVERED: usize = 2_000;
    const MAX_SIZED: usize = 2_000;
    let mut last_flush = Instant::now();
    let mut done_emitted = false;
    'pump: loop {
        let event = match handle
            .events
            .recv_timeout(std::time::Duration::from_millis(BATCH_WINDOW_MS))
        {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                let progress = if progress_due {
                    Some(build_progress(&handle, latest, volume_used))
                } else {
                    None
                };
                progress_due = false;
                flush(
                    &mut discovered,
                    &mut sized,
                    &mut permissions,
                    progress,
                    &mut last_flush,
                );
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
                        status: NodeStatus::Ok,
                    });
                    // A single restore can carry hundreds of thousands of nodes;
                    // never let one batch exceed the cap.
                    if discovered.len() >= MAX_DISCOVERED {
                        flush(
                            &mut discovered,
                            &mut sized,
                            &mut permissions,
                            None,
                            &mut last_flush,
                        );
                    }
                }
            }
            ScanEvent::DirectoryListed { dir, .. } => {
                // The directory itself is known and still filling in.
                sized.push(SizedInfo {
                    id: dir.key.to_string(),
                    size: Some(dir.size.dominant()),
                    pending: true,
                    status: NodeStatus::Ok,
                });
                for entry in &dir.entries {
                    // Files are not retained on the front end; the explorer
                    // fetches an open folder's files on demand (list_dir_files).
                    if !entry.is_dir {
                        continue;
                    }
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
                        status: NodeStatus::Ok,
                    });
                    if discovered.len() >= MAX_DISCOVERED {
                        flush(
                            &mut discovered,
                            &mut sized,
                            &mut permissions,
                            None,
                            &mut last_flush,
                        );
                    }
                }
            }
            ScanEvent::DirectorySized {
                key,
                size: new_size,
                pending,
                estimated,
                ..
            } => {
                sized.push(SizedInfo {
                    id: key.to_string(),
                    size: Some(new_size.dominant()),
                    pending,
                    status: if estimated {
                        NodeStatus::Estimated
                    } else {
                        NodeStatus::Ok
                    },
                });
            }
            ScanEvent::DirectoryClosed { key, status, .. } => {
                // The final size arrives through the `DirectorySized` flush;
                // publish only the terminal status.
                sized.push(SizedInfo {
                    id: key.to_string(),
                    size: None,
                    pending: false,
                    status: match status {
                        DirCloseStatus::Denied => NodeStatus::Denied,
                        DirCloseStatus::Awaiting => NodeStatus::Awaiting,
                        DirCloseStatus::Ok => NodeStatus::Ok,
                    },
                });
            }
            ScanEvent::Progress { progress, .. } => {
                latest = progress;
                if let Some(manager) = app.try_state::<ScanManager>() {
                    manager.set_file_count(progress.files);
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
                    size: Some(size.dominant()),
                    pending: false,
                    status: NodeStatus::Ok,
                });
            }
            ScanEvent::PermissionRequested { key, path, name, tcc, .. } => {
                // Ask the front end to drive an NSOpenPanel grant or a skip;
                // mark the parked node without touching its last known size.
                permissions.push(PermissionInfo {
                    id: key.to_string(),
                    path,
                    name,
                    tcc,
                });
                sized.push(SizedInfo {
                    id: key.to_string(),
                    size: None,
                    pending: false,
                    status: NodeStatus::Awaiting,
                });
            }
            ScanEvent::CalibrationFinished { .. } => {
                // Predicted-giant calibration finished; the coordinator keeps
                // lingering for authorization grants, so keep pumping.
            }
            ScanEvent::Finished {
                scan: _,
                outcome: finished,
                progress: final_progress,
            } => {
                // Main walk done. Flush every buffered discovery/size and emit
                // done so the UI presents results; the coordinator stays alive
                // for post-completion grants, so the loop keeps pumping the
                // refresh events that follow.
                outcome = finished;
                latest = final_progress;
                progress_due = false;
                flush(
                    &mut discovered,
                    &mut sized,
                    &mut permissions,
                    Some(build_progress(&handle, latest, volume_used)),
                    &mut last_flush,
                );
                let _ = app.emit(
                    "scan://done",
                    serde_json::json!({
                        "epoch": epoch,
                        "cancelled": matches!(outcome, ScanOutcome::Cancelled),
                        "root": root_text,
                    }),
                );
                done_emitted = true;
                if let Some(manager) = app.try_state::<ScanManager>() {
                    manager.running.store(false, Ordering::SeqCst);
                    manager.paused.store(false, Ordering::SeqCst);
                    manager.menu_refreshing.store(false, Ordering::SeqCst);
                }
            }
            ScanEvent::RefreshFinished { path, .. } => {
                // A post-completion grant finished patching totals; tell the
                // front end to refresh analysis without a new full scan.
                let _ = app.emit(
                    "scan://refreshed",
                    serde_json::json!({ "epoch": epoch, "path": path }),
                );
            }
            // `ScanEvent` is non-exhaustive; an unknown future variant is
            // ignored rather than mistaken for completion.
            _ => {}
        }

        // Bound payloads while events stream back-to-back: flush when the
        // coalescing window elapses or a buffer reaches its cap.
        if last_flush.elapsed() >= BATCH_WINDOW
            || discovered.len() >= MAX_DISCOVERED
            || sized.len() >= MAX_SIZED
        {
            let progress = if progress_due {
                Some(build_progress(&handle, latest, volume_used))
            } else {
                None
            };
            progress_due = false;
            flush(
                &mut discovered,
                &mut sized,
                &mut permissions,
                progress,
                &mut last_flush,
            );
        }
    }

    // Drain anything still buffered, with a final progress reading.
    flush(
        &mut discovered,
        &mut sized,
        &mut permissions,
        Some(build_progress(&handle, latest, volume_used)),
        &mut last_flush,
    );
    // Normal path: `Finished` already emitted done. Defensive fallback for an
    // unexpected early disconnect.
    if !done_emitted {
        let _ = app.emit(
            "scan://done",
            serde_json::json!({
                "epoch": epoch,
                "cancelled": matches!(outcome, ScanOutcome::Cancelled),
                "root": root_text,
            }),
        );
    }
}

fn build_progress(
    handle: &ScanHandle,
    progress: sift_core::Progress,
    volume_used: u64,
) -> ProgressInfo {
    let tracked = handle
        .tree
        .lock()
        .map(|tree| tree.node_count() as usize)
        .unwrap_or(0);
    ProgressInfo {
        files: progress.files,
        dirs: progress.dirs,
        bytes: progress.bytes.dominant(),
        total_bytes: volume_used,
        percent: progress.coverage * 100.0,
        rate_bytes_per_sec: progress.bytes_per_sec(),
        awaiting: progress.awaiting,
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

/// On-demand snapshot of a folder's entries for the explorer.
///
/// The scan tree records directories permanently, but individual file entries
/// are not retained by the front end (a full-volume scan has millions), so the
/// explorer reads the folder live when it is opened. Returns every immediate
/// entry; the front end already holds the directory children.
#[tauri::command]
pub(crate) fn list_dir_files(path: String) -> Result<Vec<NodeInfo>, String> {
    let dir = PathBuf::from(&path);
    let reader = sift_platform::dir::DirReader::read(&dir, true)
        .map_err(|err| err.to_string())?;
    let parent_key = NodeKey::from_path(&dir);
    let mut entries = Vec::new();
    for raw in reader.entries() {
        if raw.name == b"." || raw.name == b".." {
            continue;
        }
        let name = String::from_utf8_lossy(&raw.name).into_owned();
        let child_path = join_path(&path, &name);
        let key = NodeKey::from_path(Path::new(&child_path));
        entries.push(NodeInfo {
            id: key.to_string(),
            parent_id: Some(parent_key.to_string()),
            name,
            path: child_path,
            is_dir: raw.is_dir,
            size: raw.size().dominant(),
            modified_ms: Some(raw.mtime_ms).filter(|value| *value != 0),
            deletable: raw.writable_by_us(),
            pending: false,
            status: NodeStatus::Ok,
        });
    }
    Ok(entries)
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
