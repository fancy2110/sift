//! Tauri adapter for filesystem watching.
//!
//! Detects deletions made outside the app (Finder, Explorer, a terminal) and
//! publishes them as `fs://deleted`, so a stale treemap never lingers. The
//! watcher itself lives in `sift-platform`.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use sift_platform::watch::{FsEvent, FsWatcherHandle};
use tauri::{AppHandle, Emitter, State};

use crate::scanner::node_id;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletedEvent {
    id: String,
    path: String,
}

/// Owns the running watcher; replacing it stops the previous watch.
#[derive(Default)]
pub struct FsWatcherState {
    inner: Mutex<Option<FsWatcherHandle>>,
}

/// Start (or replace) a recursive watch rooted at `root`.
#[tauri::command]
pub fn watch_fs(
    app: AppHandle,
    state: State<'_, FsWatcherState>,
    root: String,
    recursive: bool,
) -> Result<(), String> {
    let (handle, receiver) = FsWatcherHandle::watch_with_mode(PathBuf::from(&root), recursive)
        .map_err(|err| err.to_string())?;

    let app_for_pump = app.clone();
    std::thread::Builder::new()
        .name("sift-tauri-watch".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                // Only removals change the tree's shape; creations and writes are
                // picked up by the next scan.
                if let FsEvent::Deleted(path) = event {
                    let text = path.to_string_lossy().into_owned();
                    let _ = app_for_pump.emit(
                        "fs://deleted",
                        DeletedEvent {
                            id: node_id(&text),
                            path: text,
                        },
                    );
                }
            }
        })
        .map_err(|err| err.to_string())?;

    *state.inner.lock().unwrap() = Some(handle);
    Ok(())
}

/// Stop watching, if a watch is active.
#[tauri::command]
pub fn unwatch_fs(state: State<'_, FsWatcherState>) {
    *state.inner.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_starts_empty_and_can_be_cleared() {
        let state = FsWatcherState::default();
        assert!(state.inner.lock().unwrap().is_none());
        *state.inner.lock().unwrap() = None;
        assert!(state.inner.lock().unwrap().is_none());
    }
}
