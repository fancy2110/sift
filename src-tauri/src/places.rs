//! Tauri adapter for quick scan locations.
//!
//! The physical disks come from `list_volumes`; this adds the quick places on
//! the user's own volume (home, downloads, desktop, movies, trash). The paths
//! are real, but labels stay translation keys so the UI owns the copy.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceInfo {
    /// Stable id derived from the path.
    pub id: String,
    /// Translation key for the label.
    pub label_key: String,
    /// Icon token matching the front end's Icon set.
    pub icon: String,
    /// Absolute path to scan.
    pub path: String,
    /// Owning volume id (system volume for every quick place).
    pub volume_id: String,
}

/// List the quick places that exist on this machine.
#[tauri::command]
pub fn list_places() -> Vec<PlaceInfo> {
    let system_volume = sift_platform::volume::list_volumes()
        .into_iter()
        .find(|volume| volume.is_system());
    let volume_id = system_volume
        .map(|volume| volume.id.to_string())
        .unwrap_or_default();

    let mut places: Vec<PlaceInfo> = Vec::new();

    let mut push = |path: Option<PathBuf>, label_key: &str, icon: &str| {
        if let Some(path) = path.filter(|path| path.exists()) {
            places.push(PlaceInfo {
                id: sift_core::NodeKey::from_path(&path).to_string(),
                label_key: label_key.to_string(),
                icon: icon.to_string(),
                path: path.to_string_lossy().into_owned(),
                volume_id: volume_id.clone(),
            });
        }
    };

    push(dirs::home_dir(), "place.home", "home");
    push(dirs::download_dir(), "place.downloads", "download");
    push(dirs::desktop_dir(), "place.desktop", "desktop");
    push(dirs::video_dir(), "place.movies", "film");
    push(trash_dir(), "place.trash", "trash");

    places
}

/// The user's OS trash directory.
#[cfg(target_os = "macos")]
fn trash_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".Trash"))
}

#[cfg(target_os = "windows")]
fn trash_dir() -> Option<PathBuf> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn trash_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
        .map(|data| data.join("Trash"))
        .filter(|trash| trash.exists())
}
