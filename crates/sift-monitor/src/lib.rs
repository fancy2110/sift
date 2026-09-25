//! Sift's long-running disk-space watch.
//!
//! The product's third requirement, as a capability with one job: notice when a
//! volume is running out of room and do something useful about it, safely.
//!
//! ```text
//!   sample free space ──► policy::evaluate ──► Quiet
//!        (every N s)        (pure function)     Notify
//!                              ▲               NeedsConfirmation
//!                              │               AutoClean ──► verify ──► trash
//!         store cleanable list ┘
//! ```
//!
//! The design is a deliberate hourglass: [`policy`] is a pure function holding
//! every rule that decides whether Sift may delete something unattended, and
//! [`monitor`] is the thin thread that samples, asks, and obeys. There is no
//! second route to a deletion.
//!
//! What unattended cleanup can never do:
//!
//! * touch anything that is not `Safe` **and** explicitly approved;
//! * touch anything outside the user's home while `home_only` is set;
//! * proceed when an entry changed since it was approved (the fingerprint is
//!   re-checked immediately before the move);
//! * delete permanently — everything goes to the OS trash;
//! * run again inside the cooldown window, or reclaim more than
//!   `max_auto_clean_bytes` without asking.

pub mod monitor;
pub mod policy;

pub use monitor::{
    clean_now, CleanupOutcome, CleanupSource, Monitor, MonitorControl, MonitorEvent, MonitorHandle,
    Remover, SystemTrash,
};
pub use policy::{
    evaluate, inside_home, paths_of, still_matches, AlertLevel, ConfirmationReason, DiskSample,
    MonitorAction, MonitorConfig, MonitorDecision,
};
