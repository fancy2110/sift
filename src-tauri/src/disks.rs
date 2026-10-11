//! Tauri adapter for volume enumeration.
//!
//! The platform work lives in `sift-platform`; this only reshapes it for the
//! front end's `camelCase` contract.

use serde::Serialize;
use sift_core::Volume;
use tauri::command;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    /// Stable id derived from the mount point.
    pub id: String,
    /// Human-friendly volume name.
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub file_system: String,
}

impl From<&Volume> for VolumeInfo {
    fn from(volume: &Volume) -> Self {
        Self {
            id: volume.id.to_string(),
            name: volume.name.clone(),
            mount_point: volume.mount_point.to_string_lossy().into_owned(),
            total_bytes: volume.total_bytes,
            available_bytes: volume.available_bytes,
            is_removable: volume.is_removable,
            file_system: volume.file_system.clone(),
        }
    }
}

/// List every mounted volume, system volume first.
///
/// Async and on a blocking-task thread on purpose: `sysinfo` refreshes every
/// disk's statistics (statfs/quota calls per mount), which takes tens of
/// milliseconds — and far longer with a hung network mount. Tauri runs a
/// synchronous command on the platform main thread, so an inline enumeration
/// stalls window events during startup's first IPC wave.
#[command]
pub async fn list_volumes() -> Result<Vec<VolumeInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        sift_platform::volume::list_volumes()
            .iter()
            .map(VolumeInfo::from)
            .collect()
    })
    .await
    .map_err(|err| err.to_string())
}

/// The current user's home directory, or null when the platform cannot
/// determine it. The front end uses it for home-scoped UI; the backend's
/// safety checks keep resolving the home directory independently.
#[command]
pub fn home_dir() -> Option<String> {
    dirs::home_dir().map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_are_reshaped_without_loss() {
        let volumes = sift_platform::volume::list_volumes();
        assert!(
            !volumes.is_empty(),
            "a real machine has at least one volume"
        );
        let mapped: Vec<VolumeInfo> = volumes.iter().map(VolumeInfo::from).collect();
        assert_eq!(mapped.len(), volumes.len());
        for (source, info) in volumes.iter().zip(mapped.iter()) {
            assert_eq!(info.id, source.id.to_string());
            assert_eq!(info.mount_point, source.mount_point.to_string_lossy());
            assert_eq!(info.total_bytes, source.total_bytes);
            assert_eq!(info.available_bytes, source.available_bytes);
            assert_eq!(info.is_removable, source.is_removable);
            assert_eq!(info.file_system, source.file_system);
        }
        // The system volume must sort first, which the front end relies on.
        assert!(volumes[0].is_system());
    }
}
