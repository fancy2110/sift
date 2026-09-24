//! Filesystem watcher: detects deletions made outside the app
//! (Finder/Explorer/terminal) and streams the removed paths + node ids.

use std::path::PathBuf;
use std::sync::Mutex;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::scanner::node_id;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletedEvent {
    id: String,
    path: String,
}

pub struct FsWatcherState {
    inner: Mutex<Option<notify::RecommendedWatcher>>,
}

impl Default for FsWatcherState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

/// Start (or replace) a recursive watch rooted at `root`.
#[tauri::command]
pub fn watch_fs(
    app: AppHandle,
    state: tauri::State<'_, FsWatcherState>,
    root: String,
) -> Result<(), String> {
    let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Remove(_) | EventKind::Any) {
                // On rename-away the second half may arrive as Remove;
                // treat all removed paths as deletions.
                for path in dedup(&event.paths) {
                    let p = path.to_string_lossy().to_string();
                    let _ = app.emit(
                        "fs://deleted",
                        DeletedEvent {
                            id: node_id(&p),
                            path: p,
                        },
                    );
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;

    let mut guard = state.inner.lock().unwrap();
    let mut w = watcher;
    w.watch(&PathBuf::from(&root), RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    *guard = Some(w);
    Ok(())
}

fn dedup(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in paths {
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
    out
}
