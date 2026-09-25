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

pub use dir::{DirReader, RawEntry, ReaderKind};
pub use trash::{trash_paths, TrashResult};
pub use volume::{device_id, is_same_volume, list_volumes, system_volume};
pub use walk::walk_dirs;
pub use watch::{watch_root, FsEvent, FsWatcherHandle};
