//! Sift platform capabilities.
//!
//! The thin layer between the pure domain ([`sift_core`]) and the operating
//! system. Each capability here is a narrow seam with a portable contract and
//! a per-platform implementation:
//!
//! * [`volume`] — mounted volumes and their capacity;
//! * [`dir`] — bulk directory reading with the fastest available syscalls;
//! * [`trash`] — move files to the OS trash / recycle bin;
//! * [`watch`] — filesystem change notifications;
//! * [`walk`] — a thread-pool directory walker that yields raw entries
//!   without touching the tree.
//!
//! The scan engine and both front ends depend on these seams and never call
//! `std::fs` directly for volume or deletion work.

pub mod dir;
pub mod trash;
pub mod volume;
pub mod walk;
pub mod watch;

pub use dir::{DirReader, RawEntry, ReadErrorKind, ReaderKind};
pub use trash::{trash_paths, TrashResult};
pub use volume::{device_id, is_same_volume, list_volumes, system_volume};
pub use walk::walk_dirs;
pub use watch::{watch_root, FsEvent, FsWatcherHandle};

/// Number of worker threads the scan engine should use by default.
///
/// Directory walking is syscall-bound. On asymmetric CPUs scheduling the pool
/// across the efficiency cluster both slows the scan and steals the cores the
/// rest of the system needs, so this reports the *performance* core count on
/// macOS and the available parallelism elsewhere. Callers that pin a worker
/// count via [`sift_core::ScanPolicy`] override it.
pub fn recommended_scan_workers() -> usize {
    let cores = performance_core_count().unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4)
    });
    cores.clamp(2, 16)
}

/// Physical performance-core count on Apple silicon (`P` cluster). `None` on
/// symmetric or non-Apple platforms, which fall back to available parallelism.
#[cfg(target_os = "macos")]
fn performance_core_count() -> Option<usize> {
    use std::ffi::CString;

    extern "C" {
        fn sysctlbyname(
            name: *const std::os::raw::c_char,
            oldp: *mut u8,
            oldlenp: *mut usize,
            newp: *mut u8,
            newlen: usize,
        ) -> std::os::raw::c_int;
    }

    let key = CString::new("hw.perflevel0.physicalcpu").ok()?;
    let mut value: i64 = 0;
    let mut length = std::mem::size_of::<i64>();
    let rc = unsafe {
        sysctlbyname(
            key.as_ptr(),
            &mut value as *mut i64 as *mut u8,
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 && value > 0 {
        Some(value as usize)
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn performance_core_count() -> Option<usize> {
    None
}

#[cfg(test)]
mod volume_probe {
    #[test]
    fn print_the_real_volumes() {
        let volumes = super::list_volumes();
        println!("volumes: {}", volumes.len());
        for volume in &volumes {
            println!(
                "  id={:?} name={:?} mount={:?} total={} avail={} removable={} fs={:?}",
                volume.id,
                volume.name,
                volume.mount_point,
                volume.total_bytes,
                volume.available_bytes,
                volume.is_removable,
                volume.file_system
            );
        }
        println!(
            "system_volume: {:?}",
            super::system_volume(&volumes).map(|v| v.name.clone())
        );
        println!(
            "free_space(/): {:?}",
            crate::volume::free_space(std::path::Path::new("/"))
        );
        println!(
            "free_space(/Users): {:?}",
            crate::volume::free_space(std::path::Path::new("/Users"))
        );
    }
}
