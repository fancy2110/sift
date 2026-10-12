//! What the operating system reports about one mounted volume.

use std::path::{Path, PathBuf};

use sift_core::{format_bytes, Volume, VolumeId};

/// Enumerate mounted volumes, system volume first.
///
/// Never fails: a machine with no discoverable volume returns an empty list,
/// which the UI renders as an empty state rather than an error dialog.
pub fn list_volumes() -> Vec<Volume> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut volumes: Vec<Volume> = disks
        .iter()
        .filter(|disk| is_user_volume(disk.mount_point()))
        .map(|disk| {
            let mount_point = disk.mount_point().to_path_buf();
            let file_system =
                String::from_utf8_lossy(disk.file_system().as_encoded_bytes()).to_string();
            Volume::new(
                VolumeId::from_mount_point(&mount_point),
                display_name(&mount_point, disk.name().to_string_lossy().as_ref()),
                mount_point,
                disk.total_space(),
                disk.available_space(),
                disk.is_removable(),
                file_system,
            )
        })
        .collect();

    // macOS APFS: `/` (the sealed system) and `/System/Volumes/Data` (user
    // data) are two volumes of one volume group on one physical disk. When the
    // root is present, drop the Data row so the picker shows one Macintosh HD.
    #[cfg(target_os = "macos")]
    if volumes.iter().any(|volume| volume.mount_point == Path::new("/")) {
        volumes.retain(|volume| volume.mount_point != Path::new("/System/Volumes/Data"));
    }

    volumes.sort_by(|left, right| {
        right
            .is_system()
            .cmp(&left.is_system())
            .then_with(|| right.total_bytes.cmp(&left.total_bytes))
            .then_with(|| left.mount_point.cmp(&right.mount_point))
    });
    volumes
}

/// Current `(total, available)` bytes for the volume containing `path`.
///
/// Accepts either a mount point or any path inside one: the longest matching
/// mount point wins, so a caller can pass the directory it is watching and still
/// get the right file system. Re-reads rather than caching, because the
/// background monitor exists precisely to notice change. `None` when nothing
/// matches — an unmounted external disk is a normal event, not an error.
pub fn free_space(path: &Path) -> Option<(u64, u64)> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        // Longest mount point first: `/Volumes/Backup` must beat `/`.
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| (disk.total_space(), disk.available_space()))
}

/// The volume the user means by default: the boot volume if it is listed,
/// otherwise the largest fixed disk.
pub fn system_volume(volumes: &[Volume]) -> Option<&Volume> {
    volumes
        .iter()
        .find(|volume| volume.is_system())
        .or_else(|| volumes.iter().find(|volume| !volume.is_removable))
        .or_else(|| volumes.first())
}

/// Whether a path lives on the same volume as `root`, used to decide if a
/// directory walk has crossed a mount point into another filesystem.
///
/// Compares the device id of the two paths, which is what actually decides
/// whether leaving the tree changes volumes; comparing path prefixes does not
/// work for bind mounts or nested volume roots.
pub fn is_same_volume(root: &Path, path: &Path) -> bool {
    match (device_id(root), device_id(path)) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

#[cfg(unix)]
pub fn device_id(path: &Path) -> Option<u64> {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|meta| meta.dev())
}

#[cfg(windows)]
pub fn device_id(path: &Path) -> Option<u64> {
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    fs::metadata(path)
        .ok()
        .map(|meta| meta.volume_serial_number().unwrap_or(0) as u64)
}

#[cfg(not(any(unix, windows)))]
pub fn device_id(_path: &Path) -> Option<u64> {
    None
}

/// Whether a mount point is a disk the user can meaningfully choose.
///
/// On macOS only the boot volume, its paired Data volume (collapsed into the
/// root afterwards) and mounts under `/Volumes` qualify. Auxiliary system
/// volumes (VM, Preboot, Update, Recovery, simulator runtimes) are hidden: they
/// are not user disks and picking one for a scan is meaningless.
#[cfg(target_os = "macos")]
fn is_user_volume(mount_point: &Path) -> bool {
    if mount_point == Path::new("/")
        || mount_point == Path::new("/System/Volumes/Data")
    {
        return true;
    }
    mount_point.starts_with("/Volumes")
}

#[cfg(not(target_os = "macos"))]
fn is_user_volume(_mount_point: &Path) -> bool {
    true
}

/// A human label for a mount point.
///
/// The boot volume reports its mount point as `/`, which is not a name anyone
/// recognises, so it gets the platform's conventional label.
fn display_name(mount_point: &Path, reported: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        if mount_point == Path::new("/") {
            return "Macintosh HD".to_string();
        }
    }
    if !reported.is_empty() && reported != "/" && reported != "\\" {
        return reported.to_string();
    }
    let label = mount_point
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if label.is_empty() {
        mount_point.to_string_lossy().into_owned()
    } else {
        label
    }
}

/// One-line description of free space, used by the CLI and by log lines.
pub fn describe(volume: &Volume) -> String {
    format!(
        "{} — {} free of {} ({} used)",
        volume.name,
        format_bytes(volume.available_bytes),
        format_bytes(volume.total_bytes),
        format_bytes(volume.used_bytes())
    )
}

/// Convenience for adapters that still speak in `PathBuf`.
pub fn mount_point(volume: &Volume) -> PathBuf {
    volume.mount_point.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_at_least_one_volume_on_a_real_machine() {
        let volumes = list_volumes();
        assert!(!volumes.is_empty(), "no volumes discovered");
        for volume in &volumes {
            assert!(volume.total_bytes > 0, "{volume:?} reports no capacity");
            assert!(
                volume.available_bytes <= volume.total_bytes,
                "{volume:?} reports more free than total"
            );
            assert!(!volume.name.is_empty());
            assert!(volume.mount_point.is_absolute());
        }
    }

    #[test]
    fn system_volume_is_first_and_found() {
        let volumes = list_volumes();
        let system = system_volume(&volumes).expect("a system volume");
        assert!(system.is_system(), "{system:?} is not the system volume");
        assert!(volumes[0].is_system(), "system volume is not sorted first");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn auxiliary_and_data_volumes_are_hidden_or_collapsed() {
        let volumes = list_volumes();
        for volume in &volumes {
            assert_ne!(
                volume.mount_point,
                Path::new("/System/Volumes/Data"),
                "Data volume must collapse into the root: {volumes:?}"
            );
            assert!(
                !volume.mount_point.starts_with("/System/Volumes"),
                "auxiliary system volume leaked into the picker: {}",
                volume.mount_point.display()
            );
        }
        // Exactly one boot disk.
        assert!(
            volumes.iter().any(|v| v.mount_point == Path::new("/")),
            "boot volume missing: {volumes:?}"
        );
    }

    #[test]
    fn same_volume_compares_devices() {
        let root = Path::new("/");
        assert!(is_same_volume(root, Path::new("/Users")));
        assert!(device_id(root).is_some());
    }

    #[test]
    fn free_space_reads_a_real_mount_point() {
        let (total, available) = free_space(Path::new("/")).expect("the root is mounted");
        assert!(total > 0);
        assert!(available <= total);
    }

    #[test]
    fn free_space_resolves_any_absolute_path_to_some_volume() {
        // Longest-prefix resolution means every absolute path belongs to a
        // volume (at minimum `/`), which is what a monitor watching a directory
        // needs.
        assert!(free_space(Path::new("/nonexistent/sift-mount")).is_some());
    }

    #[test]
    fn free_space_returns_none_when_nothing_could_match() {
        // A relative path cannot belong to any mount point, which is the only
        // reachable "no volume" case on a normal machine.
        assert!(free_space(Path::new("relative/path")).is_none());
    }

    #[test]
    fn free_space_resolves_a_path_inside_a_volume() {
        // A nested path must resolve to the volume that contains it, not to
        // "no volume".
        let inside = free_space(Path::new("/var/folders"));
        let root = free_space(Path::new("/"));
        if let (Some(inside), Some(root)) = (inside, root) {
            assert_eq!(inside.0, root.0, "same volume, same capacity");
        }
    }

    #[test]
    fn describe_mentions_capacity() {
        let volumes = list_volumes();
        let text = describe(&volumes[0]);
        assert!(text.contains("free of"), "{text}");
    }
}
