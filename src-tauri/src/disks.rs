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
#[command]
pub fn list_volumes() -> Vec<VolumeInfo> {
    sift_platform::volume::list_volumes()
        .iter()
        .map(VolumeInfo::from)
        .collect()
}

/// The current user's home directory. The front end uses this as the deep-scan
/// root for system volumes: system files are not cleanable.
#[command]
pub fn home_dir() -> Option<String> {
    dirs::home_dir().map(|p| p.to_string_lossy().into_owned())
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
