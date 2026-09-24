//! Cross-platform volume enumeration.

use serde::Serialize;
use sysinfo::Disks;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    /// Stable id derived from mount point.
    pub id: String,
    /// Human-friendly volume name.
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub file_system: String,
}

fn volume_id(mount: &str) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in mount.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("vol-{:x}", h)
}

/// List all mounted volumes, system volume first.
#[tauri::command]
pub fn list_volumes() -> Vec<VolumeInfo> {
    let disks = Disks::new_with_refreshed_list();
    let mut out: Vec<VolumeInfo> = disks
        .iter()
        .map(|d| {
            let mount = d.mount_point().to_string_lossy().to_string();
            let name = d.name().to_string_lossy().to_string();
            let label = volume_label(&mount, &name);
            VolumeInfo {
                id: volume_id(&mount),
                name: label,
                mount_point: mount,
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                is_removable: d.is_removable(),
                file_system: String::from_utf8_lossy(d.file_system().as_encoded_bytes())
                    .to_string(),
            }
        })
        .collect();

    sort_system_first(&mut out);
    out
}

fn volume_label(mount: &str, name: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        if mount == "/" || mount == "/System/Volumes/Data" {
            return "Macintosh HD".to_string();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = name;
        // On Windows the volume label is already meaningful; fall through.
    }
    if name.is_empty() || name == "/" {
        mount.to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_at_least_one_volume() {
        let vols = list_volumes();
        assert!(!vols.is_empty(), "must find at least one mounted volume");
        for v in &vols {
            assert!(v.total_bytes > 0);
            assert!(v.available_bytes <= v.total_bytes);
            assert!(!v.id.is_empty());
        }
        println!("{vols:#?}");
    }
}

fn sort_system_first(vols: &mut [VolumeInfo]) {
    vols.sort_by_key(|v| {
        #[cfg(target_os = "windows")]
        let system = v.mount_point.eq_ignore_ascii_case("C:\\");
        #[cfg(not(target_os = "windows"))]
        let system = v.mount_point == "/" || v.mount_point == "/System/Volumes/Data";
        if system {
            0
        } else {
            1
        }
    });
}
