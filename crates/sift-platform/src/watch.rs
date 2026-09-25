//! Filesystem change notifications.
//!
//! The app detects deletions made *outside* it (Finder, Explorer, a terminal)
//! so a stale treemap never lingers. `notify` provides a cross-platform
//! backend; the seam here keeps the rest of the code independent of it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

/// A change the app cares about, already classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum FsEvent {
    /// A path disappeared.
    Deleted(PathBuf),
    /// A path was created (files are counted on a later rescan anyway).
    Created(PathBuf),
    /// Something changed that does not map to create/delete cleanly.
    Changed(PathBuf),
}

/// Ownership handle for a running watcher; dropping it stops notifications.
///
/// Held by whoever owns the scan session (the engine or a front end).
#[derive(Debug)]
pub struct FsWatcherHandle {
    _watcher: notify::RecommendedWatcher,
}

impl FsWatcherHandle {
    /// Start watching `root` recursively and return a handle plus a receiver
    /// of classified events.
    pub fn watch(root: PathBuf) -> std::io::Result<(Self, mpsc::Receiver<FsEvent>)> {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = raw_tx.send(event);
        })
        .map_err(to_io_err)?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(to_io_err)?;

        let (event_tx, event_rx) = mpsc::channel::<FsEvent>();

        // Pump the raw stream into classified events in a detached thread; the
        // channel closes when the watcher is dropped (its sender is freed).
        std::thread::spawn(move || {
            while let Ok(event) = raw_rx.recv() {
                let Ok(event) = event else { continue };
                for path in &event.paths {
                    let Some(classified) = classify_event(&event.kind, path) else {
                        continue;
                    };
                    if event_tx.send(classified).is_err() {
                        return;
                    }
                }
            }
        });

        Ok((Self { _watcher: watcher }, event_rx))
    }
}

/// Translate one raw notify event into the app's vocabulary.
fn classify_event(kind: &notify::EventKind, path: &Path) -> Option<FsEvent> {
    use notify::EventKind;
    let owned = || path.to_path_buf();
    match kind {
        EventKind::Create(_) => Some(FsEvent::Created(owned())),
        EventKind::Remove(_) => Some(FsEvent::Deleted(owned())),
        _ => Some(FsEvent::Changed(owned())),
    }
}

/// A helper that owns a watcher and drains its events into a caller-provided
/// callback, for front ends that prefer callbacks over a channel.
pub fn watch_root(
    root: PathBuf,
    on_event: impl Fn(FsEvent) + Send + 'static,
) -> std::io::Result<FsWatcherHandle> {
    let (handle, receiver) = FsWatcherHandle::watch(root)?;
    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            on_event(event);
        }
    });
    Ok(handle)
}

fn to_io_err(err: notify::Error) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_maps_notify_kinds() {
        use notify::EventKind;
        let path = PathBuf::from("/tmp/x");
        assert_eq!(
            classify_event(&EventKind::Create(notify::event::CreateKind::File), &path),
            Some(FsEvent::Created(path.clone()))
        );
        assert_eq!(
            classify_event(&EventKind::Remove(notify::event::RemoveKind::File), &path),
            Some(FsEvent::Deleted(path.clone()))
        );
        assert_eq!(
            classify_event(&EventKind::Modify(notify::event::ModifyKind::Data(notify::event::DataChange::Any)), &path),
            Some(FsEvent::Changed(path))
        );
    }

    /// End-to-end watcher check. FSEvents only fires for approved scopes, so
    /// this is ignored by default and enabled in a normal macOS session.
    #[test]
    #[ignore = "FSEvents requires real (non-sandboxed) session scopes"]
    fn watcher_detects_a_creation_and_deletion() {
        let base = std::env::temp_dir().join(format!("sift-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let (_handle, receiver) = FsWatcherHandle::watch(base.clone()).expect("watch to start");

        // FSEvents arms asynchronously; keep poking until the backend reports
        // our path, up to a generous deadline.
        let file = base.join("appear.txt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut saw_our_path = false;
        while std::time::Instant::now() < deadline {
            let _ = std::fs::write(&file, b"x");
            match receiver.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(FsEvent::Deleted(p)) | Ok(FsEvent::Created(p)) | Ok(FsEvent::Changed(p)) => {
                    if p == file {
                        saw_our_path = true;
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let _ = std::fs::remove_file(&file);
                    continue;
                }
                Err(_) => break,
            }
        }
        assert!(saw_our_path, "watcher never reported our path");

        let _ = std::fs::remove_dir_all(&base);
    }
}
