//! Sift domain core.
//!
//! Everything the product knows about a disk that is not specific to a
//! platform, a scanner implementation, or a UI toolkit lives here:
//!
//! * [`id`] — 128-bit path identity, volume identity, hardlink file identity;
//! * [`interner`] — one arena for every distinct file name;
//! * [`size`] — logical vs physical byte accounting;
//! * [`tree`] — the compact arena tree that is both the scan accumulator and
//!   the UI's model;
//! * [`scan`] — volumes, scan policy, progress, and the event contract;
//! * [`deletable`] — whether an entry may be sent to the trash, and why not.
//!
//! The crate has no dependency on any windowing toolkit or IPC layer: it is the
//! seam that lets the Tauri front end and the GPUI front end show the same disk
//! with the same numbers.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod deletable;
pub mod id;
pub mod interner;
pub mod scan;
pub mod size;
pub mod tree;

pub use deletable::{classify, classify_with_permissions, Deletable};
pub use id::{FileId, NodeKey, VolumeId};
pub use interner::{NameInterner, NameRef};
pub use scan::{
    DirectoryEntry, DirectorySummary, Progress, ScanEvent, ScanId, ScanOutcome, ScanPolicy,
    ScanRequest, Volume,
};
pub use size::{format_bytes, ByteSize};
pub use tree::{NodeKind, NodeView, ScanTree, TreeConfig, TreeStats, NONE};
