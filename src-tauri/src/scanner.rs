//! Incremental, priority-aware filesystem scanner.
//!
//! Scan order adapts to the user's current location:
//!   1. focused directory
//!   2. its ancestor chain
//!   3. sibling directories
//!   4. descendants, then everything else (BFS)
//!
//! Events are streamed to the frontend as nodes appear and sizes settle,
//! so the current directory is interactive long before the scan finishes.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// Maximum directory nodes we retain metadata for (memory budget).
/// Sizes are still accumulated fully beyond this limit.
const MAX_TRACKED_NODES: usize = 250_000;
/// Directories deeper than this are size-counted but not detailed.
const MAX_DEPTH: usize = 18;
/// Emit interim size updates at most this often.
const INTERIM_FLUSH_MS: u128 = 250;

// ---- public DTOs -----------------------------------------------------------

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
    /// Still being scanned (directories only).
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressInfo {
    pub files: u64,
    pub dirs: u64,
    pub tracked_nodes: usize,
}

// ---- stable ids ------------------------------------------------------------

pub fn node_id(path: &str) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in path.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("n-{:x}", h)
}

// ---- deletability ----------------------------------------------------------

#[cfg(unix)]
pub fn is_deletable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(meta) = fs::metadata(parent) else {
        return false;
    };
    let mode = meta.mode();
    if meta.uid() == libc_euid() {
        mode & 0o200 != 0
    } else {
        mode & 0o002 != 0
    }
}

#[cfg(unix)]
fn libc_euid() -> u32 {
    // Avoid an extra libc dependency for a single call.
    extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

#[cfg(windows)]
pub fn is_deletable(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    // Protected system locations are not user-deletable.
    const PROTECTED: [&str; 4] = [
        "c:\\windows\\",
        "c:\\program files\\",
        "c:\\program files (x86)\\",
        "c:\\programdata\\",
    ];
    !PROTECTED.iter().any(|pref| p.starts_with(pref))
}

// ---- priority queue --------------------------------------------------------

#[derive(Clone)]
struct Queued {
    path: PathBuf,
    depth: usize,
    seq: u64,
}

/// Category drives ordering; lower is sooner.
fn category(path: &Path, depth: usize, focus: &Path) -> u8 {
    if path == focus {
        0
    } else if focus.starts_with(path) {
        1 // ancestor: closer (deeper) ancestors win via depth
    } else if path.parent() == focus.parent() {
        2 // sibling of focus
    } else if path.starts_with(focus) {
        3
    } else {
        let _ = depth;
        4
    }
}

#[derive(Clone)]
struct OrdItem {
    q: Queued,
    cat: u8,
}

impl PartialEq for OrdItem {
    fn eq(&self, other: &Self) -> bool {
        self.q.seq == other.q.seq
    }
}
impl Eq for OrdItem {}
impl PartialOrd for OrdItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; "best" is smallest (cat, then...)
        match other.cat.cmp(&self.cat) {
            Ordering::Equal => {}
            o => return o,
        }
        match other.q.depth.cmp(&self.q.depth) {
            // Within ancestor category prefer DEEPER ancestor first.
            Ordering::Equal => {}
            o if self.cat == 1 => return o.reverse(),
            o => return o,
        }
        other.q.seq.cmp(&self.q.seq)
    }
}

struct Pending {
    queued: HashSet<PathBuf>,
    heap: BinaryHeap<OrdItem>,
}

impl Pending {
    fn new() -> Self {
        Self {
            queued: HashSet::new(),
            heap: BinaryHeap::new(),
        }
    }

    fn push(&mut self, q: Queued, focus: &Path) {
        if self.queued.insert(q.path.clone()) {
            let cat = category(&q.path, q.depth, focus);
            self.heap.push(OrdItem { q, cat });
        }
    }

    fn pop(&mut self) -> Option<Queued> {
        while let Some(item) = self.heap.pop() {
            if self.queued.remove(&item.q.path) {
                return Some(item.q);
            }
        }
        None
    }

    fn rebuild(&mut self, focus: &Path) {
        let items: Vec<OrdItem> = self.heap.drain().chain(std::iter::empty()).collect();
        // Stale heap items: re-add only those still queued.
        for mut item in items {
            if self.queued.contains(&item.q.path) {
                item.cat = category(&item.q.path, item.q.depth, focus);
                self.heap.push(item);
            }
        }
    }
}

// ---- per-directory accounting ----------------------------------------------

#[derive(Default)]
struct DirAcc {
    /// Direct children not yet finished (files count instantly, dirs at completion).
    outstanding: u64,
    interim: u64,
    dirty: bool,
}

// ---- manager (Tauri state) -------------------------------------------------

pub struct ScanManager {
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    focus: Arc<Mutex<PathBuf>>,
}

impl Default for ScanManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanManager {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            focus: Arc::new(Mutex::new(PathBuf::from("/"))),
        }
    }
}

// ---- commands --------------------------------------------------------------

#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    manager: State<'_, ScanManager>,
    root: String,
    focus: String,
) -> Result<(), String> {
    if manager.running.load(AtomicOrdering::SeqCst) {
        return Err("scan already running".into());
    }
    manager.cancel.store(false, AtomicOrdering::SeqCst);
    manager.running.store(true, AtomicOrdering::SeqCst);
    *manager.focus.lock().unwrap() = PathBuf::from(&focus);

    let cancel = manager.cancel.clone();
    let running = manager.running.clone();
    let focus_arc = manager.focus.clone();
    let root = PathBuf::from(root);
    let app2 = app.clone();

    std::thread::spawn(move || {
        scan_loop(app2, root, cancel, focus_arc);
        running.store(false, AtomicOrdering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
pub fn set_scan_focus(manager: State<'_, ScanManager>, focus: String) {
    *manager.focus.lock().unwrap() = PathBuf::from(focus);
}

#[tauri::command]
pub fn cancel_scan(manager: State<'_, ScanManager>) {
    manager.cancel.store(true, AtomicOrdering::SeqCst);
}

/// Rebuild the heap when focus changes. Called from the worker; the channel
/// between command and worker is the shared focus path plus a generation flag.
fn focus_changed(manager_focus: &Arc<Mutex<PathBuf>>, last: &mut PathBuf) -> bool {
    let cur = manager_focus.lock().unwrap();
    if *cur != *last {
        *last = cur.clone();
        true
    } else {
        false
    }
}

// ---- core loop -------------------------------------------------------------

fn emit_discovered(app: &AppHandle, node: &NodeInfo) {
    let _ = app.emit("scan://discovered", node);
}

fn emit_sized(app: &AppHandle, id: &str, size: u64, pending: bool) {
    let _ = app.emit(
        "scan://sized",
        serde_json::json!({ "id": id, "size": size, "pending": pending }),
    );
}

fn scan_loop(
    app: AppHandle,
    root: PathBuf,
    cancel: Arc<AtomicBool>,
    focus_arc: Arc<Mutex<PathBuf>>,
) {
    let mut pending = Pending::new();
    let mut accs: HashMap<String, DirAcc> = HashMap::new();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    let mut tracked = 0usize;
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut seq = 0u64;
    let mut last_focus = focus_arc.lock().unwrap().clone();
    let mut last_flush = Instant::now();

    // Emit the root immediately — the UI renders it at once.
    let root_str = root.to_string_lossy().to_string();
    let root_id = node_id(&root_str);
    let root_name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| root_str.clone());
    let root_deletable = is_deletable(&root);
    parent_of.insert(root_id.clone(), None);
    accs.insert(root_id.clone(), DirAcc::default());
    emit_discovered(
        &app,
        &NodeInfo {
            id: root_id.clone(),
            parent_id: None,
            name: root_name,
            path: root_str.clone(),
            is_dir: true,
            size: 0,
            modified_ms: None,
            deletable: root_deletable,
            pending: true,
        },
    );

    pending.push(
        Queued {
            path: root.clone(),
            depth: 0,
            seq: seq,
        },
        &last_focus,
    );

    while let Some(q) = pending.pop() {
        if cancel.load(AtomicOrdering::SeqCst) {
            break;
        }
        if focus_changed(&focus_arc, &mut last_focus) {
            pending.rebuild(&last_focus);
        }

        let dir_str = q.path.to_string_lossy().to_string();
        let dir_id = node_id(&dir_str);
        let read = match fs::read_dir(&q.path) {
            Ok(rd) => rd,
            Err(_) => {
                // Unreadable directory: settle it at zero.
                finish_dir(&app, &mut accs, &mut parent_of, &dir_id, None, &cancel);
                continue;
            }
        };

        // Gather entries (a single read_dir call; fine on all platforms).
        let mut entries: Vec<(PathBuf, fs::Metadata)> = Vec::new();
        for ent in read.flatten() {
            if let Ok(meta) = ent.metadata() {
                entries.push((ent.path(), meta));
            }
        }
        // Folders first helps parents settle in a sensible order; sizes stay exact.
        entries.sort_by_key(|(_, m)| if m.is_dir() { 0 } else { 1 });

        if let Some(a) = accs.get_mut(&dir_id) {
            a.outstanding = entries.len() as u64;
        }

        for (path, meta) in entries {
            if cancel.load(AtomicOrdering::SeqCst) {
                break;
            }
            seq += 1;
            let path_str = path.to_string_lossy().to_string();
            let id = node_id(&path_str);
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.clone());
            let is_dir = meta.is_dir();
            let modified_ms = mod_ms(&meta);
            let deletable = is_deletable(&path);

            parent_of.insert(id.clone(), Some(dir_id.clone()));

            if is_dir {
                dirs += 1;
                accs.entry(id.clone()).or_default();
                if tracked < MAX_TRACKED_NODES && q.depth + 1 <= MAX_DEPTH {
                    tracked += 1;
                    emit_discovered(
                        &app,
                        &NodeInfo {
                            id: id.clone(),
                            parent_id: Some(dir_id.clone()),
                            name,
                            path: path_str,
                            is_dir: true,
                            size: 0,
                            modified_ms,
                            deletable,
                            pending: true,
                        },
                    );
                    pending.push(
                        Queued {
                            path,
                            depth: q.depth + 1,
                            seq,
                        },
                        &last_focus,
                    );
                } else {
                    // Beyond the memory/depth budget: still scan for size,
                    // but stream as a single collapsed placeholder.
                    emit_discovered(
                        &app,
                        &NodeInfo {
                            id: id.clone(),
                            parent_id: Some(dir_id.clone()),
                            name,
                            path: path_str,
                            is_dir: true,
                            size: 0,
                            modified_ms,
                            deletable,
                            pending: true,
                        },
                    );
                    // Recurse directly (still priority-ordered below siblings).
                    scan_subtree_size(&path, &id, &app, &mut accs, &mut parent_of, &cancel);
                }
            } else {
                files += 1;
                let size = meta.len();
                emit_discovered(
                    &app,
                    &NodeInfo {
                        id: id.clone(),
                        parent_id: Some(dir_id.clone()),
                        name,
                        path: path_str,
                        is_dir: false,
                        size,
                        modified_ms,
                        deletable,
                        pending: false,
                    },
                );
                contribute(&mut accs, &dir_id, size);
                settle_chain(&app, &mut accs, &parent_of, &dir_id, &cancel);
            }

            if last_flush.elapsed().as_millis() >= INTERIM_FLUSH_MS {
                flush_interim(&app, &mut accs);
                let _ = app.emit(
                    "scan://progress",
                    ProgressInfo {
                        files,
                        dirs,
                        tracked_nodes: tracked,
                    },
                );
                last_flush = Instant::now();
            }
        }

        // Directory may complete immediately (empty or all files).
        settle_chain(&app, &mut accs, &parent_of, &dir_id, &cancel);
    }

    // Settle anything still open (cancellation or read errors).
    let ids: Vec<String> = accs.keys().cloned().collect();
    for id in ids {
        if let Some(a) = accs.get(&id) {
            if a.outstanding == 0 {
                finish_dir(&app, &mut accs, &mut parent_of, &id, None, &cancel);
            }
        }
    }

    let _ = app.emit(
        "scan://progress",
        ProgressInfo {
            files,
            dirs,
            tracked_nodes: tracked,
        },
    );
    let _ = app.emit(
        "scan://done",
        serde_json::json!({ "cancelled": cancel.load(AtomicOrdering::SeqCst), "root": root_str }),
    );
}

/// Recursively measure a subtree that exceeds the tracking budget.
fn scan_subtree_size(
    path: &Path,
    id: &str,
    app: &AppHandle,
    accs: &mut HashMap<String, DirAcc>,
    parent_of: &mut HashMap<String, Option<String>>,
    cancel: &Arc<AtomicBool>,
) {
    if cancel.load(AtomicOrdering::SeqCst) {
        return;
    }
    let Ok(rd) = fs::read_dir(path) else {
        if let Some(pid) = parent_of.get(id).cloned().flatten() {
            contribute(accs, &pid, 0);
            settle_chain(app, accs, parent_of, &pid, cancel);
        }
        return;
    };
    let mut total = 0u64;
    for ent in rd.flatten() {
        if cancel.load(AtomicOrdering::SeqCst) {
            break;
        }
        if let Ok(meta) = ent.metadata() {
            if meta.is_dir() {
                let cpath = ent.path();
                let cstr = cpath.to_string_lossy().to_string();
                let cid = node_id(&cstr);
                parent_of.insert(cid.clone(), Some(id.to_string()));
                accs.entry(cid.clone()).or_default();
                scan_subtree_size(&cpath, &cid, app, accs, parent_of, cancel);
            } else {
                total += meta.len();
            }
        }
    }
    emit_sized(app, id, total, false);
    if let Some(pid) = parent_of.get(id).cloned().flatten() {
        contribute(accs, &pid, total);
        settle_chain(app, accs, parent_of, &pid, cancel);
    }
}

#[cfg(unix)]
fn mod_ms(meta: &fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.mtime())
}

#[cfg(windows)]
fn mod_ms(meta: &fs::Metadata) -> Option<i64> {
    use std::os::windows::fs::MetadataExt;
    meta.last_write_time().map(|ft| {
        // Windows FILETIME (100ns ticks since 1601) → unix ms.
        ((ft as i128 - 116_444_736_000_000_000) / 10_000) as i64
    })
}

fn contribute(accs: &mut HashMap<String, DirAcc>, dir_id: &str, size: u64) {
    if let Some(a) = accs.get_mut(dir_id) {
        a.interim += size;
        a.dirty = true;
        if a.outstanding > 0 {
            a.outstanding -= 1;
        }
    }
}

/// Walk upward while directories have no outstanding children.
fn settle_chain(
    app: &AppHandle,
    accs: &mut HashMap<String, DirAcc>,
    parent_of: &HashMap<String, Option<String>>,
    start: &str,
    cancel: &Arc<AtomicBool>,
) {
    let mut cur = start.to_string();
    loop {
        let ready = accs.get(&cur).map(|a| a.outstanding == 0).unwrap_or(false);
        if !ready {
            return;
        }
        let size = accs.get(&cur).map(|a| a.interim).unwrap_or(0);
        emit_sized(app, &cur, size, false);
        accs.remove(&cur);
        match parent_of.get(&cur).cloned().flatten() {
            Some(pid) => {
                contribute(accs, &pid, size);
                cur = pid;
            }
            None => return,
        }
        if cancel.load(AtomicOrdering::SeqCst) {
            return;
        }
    }
}

/// Force-complete a directory (used on read errors).
fn finish_dir(
    app: &AppHandle,
    accs: &mut HashMap<String, DirAcc>,
    parent_of: &mut HashMap<String, Option<String>>,
    id: &str,
    _extra: Option<u64>,
    cancel: &Arc<AtomicBool>,
) {
    settle_chain(app, accs, parent_of, id, cancel);
}

fn flush_interim(app: &AppHandle, accs: &mut HashMap<String, DirAcc>) {
    let dirty_ids: Vec<String> = accs
        .iter()
        .filter_map(|(id, a)| a.dirty.then(|| id.clone()))
        .collect();
    for id in dirty_ids {
        let a = accs.get(&id).unwrap();
        emit_sized(app, &id, a.interim, a.outstanding > 0);
        accs.get_mut(&id).unwrap().dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable() {
        assert_eq!(node_id("/a/b"), node_id("/a/b"));
        assert_ne!(node_id("/a/b"), node_id("/a/c"));
    }

    #[test]
    fn priority_ordering() {
        let focus = Path::new("/root/a/focus");
        assert_eq!(category(Path::new("/root/a/focus"), 2, focus), 0);
        assert_eq!(category(Path::new("/root/a"), 1, focus), 1);
        assert_eq!(category(Path::new("/root/a/sib"), 2, focus), 2);
        assert_eq!(category(Path::new("/root/a/focus/sub"), 3, focus), 3);
        assert_eq!(category(Path::new("/other"), 1, focus), 4);
    }

    #[test]
    fn deletability_real_tmp() {
        let tmp = std::env::temp_dir().join(format!("sift-del-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        assert!(is_deletable(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }
}
