//! Sift scan engine: fast, memory-bounded, focus-first disk walking.
//!
//! * [`ScanEngine`] starts scans; each [`ScanHandle`] streams [`ScanEvent`]s
//!   and exposes the live [`ScanTree`].
//! * [`priority`] defines the focus-first ordering that keeps the user's
//!   current directory usable within milliseconds.
//!
//! The engine depends only on [`sift_core`] (the model) and [`sift_platform`]
//! (bulk directory reading); it has no idea which front end is attached.

mod engine;
pub mod priority;

pub use engine::{ScanControl, ScanEngine, ScanHandle};
pub use priority::{category, JobKind};
