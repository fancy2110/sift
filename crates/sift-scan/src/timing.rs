//! Lightweight per-phase timing for the scan engine.
//!
//! Counters are shared atomics so workers and the coordinator can accumulate
//! time without locks. On an I/O-bound walk the numbers diagnose where the
//! wall time went: raw directory reads, snapshot probes, or the coordinator's
//! per-entry bookkeeping.
//!
//! Overhead is one relaxed atomic add per phase, so instrumentation is left in
//! production paths; reports are printed only when `SIFT_SCAN_TIMINGS=1`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const NANOS_PER_MILLIS: f64 = 1_000_000.0;

/// Accumulated time spent in each scan phase.
#[derive(Debug, Default)]
pub struct ScanTimings {
    /// Worker: raw bulk directory reads (`getattrlistbulk` / read_dir).
    pub dir_read: AtomicU64,
    /// Worker: the single fused self-stat of a directory (size + mtime)
    /// before reading it.
    pub dir_stat: AtomicU64,
    /// Worker: snapshot store lookup (includes the writer flush wait).
    pub snapshot_lookup: AtomicU64,
    /// Worker: reused directories returned from the probe.
    pub reused_dirs: AtomicU64,
    /// Worker: predicted giant directories deferred for calibration.
    pub predicted_dirs: AtomicU64,
    /// Coordinator: building per-entry paths and hashing keys.
    pub coord_path_key: AtomicU64,
    /// Coordinator: tree mutation (open dirs, record files, close cascade).
    pub coord_tree: AtomicU64,
    /// Coordinator: assembling the `DirectoryListed` payload.
    pub coord_entries: AtomicU64,
    /// Coordinator: sending events on the channel.
    pub coord_event_send: AtomicU64,
    /// Slowest single directory read, in micros (for finding stragglers).
    pub slowest_dir_us: AtomicU64,
    /// Slowest directory path, recorded alongside the time.
    pub slowest_path: Mutex<String>,
}

/// A cheap, shareable handle to the timings.
pub type TimingsHandle = Arc<ScanTimings>;

impl ScanTimings {
    pub fn new() -> TimingsHandle {
        Arc::new(Self::default())
    }

    fn add(&self, cell: &AtomicU64, elapsed: std::time::Duration) {
        cell.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Render the accumulated times as multi-line diagnostics, in millis.
    pub fn report(&self) -> String {
        let ms = |cell: &AtomicU64| cell.load(Ordering::Relaxed) as f64 / NANOS_PER_MILLIS;
        format!(
            "scan timings (ms): dir_read={:.1} dir_stat={:.1} snapshot_lookup={:.1} \
             coord_path_key={:.1} coord_tree={:.1} coord_entries={:.1} coord_event_send={:.1}; \
             reused_dirs={} slowest_dir={:.2}ms at {}",
            ms(&self.dir_read),
            ms(&self.dir_stat),
            ms(&self.snapshot_lookup),
            ms(&self.coord_path_key),
            ms(&self.coord_tree),
            ms(&self.coord_entries),
            ms(&self.coord_event_send),
            self.reused_dirs.load(Ordering::Relaxed),
            self.slowest_dir_us.load(Ordering::Relaxed) as f64 / 1000.0,
            self.slowest_path.lock().unwrap(),
        )
    }

    /// Record one directory read duration, tracking the slowest seen.
    pub fn observe_dir(&self, path: &str, elapsed: std::time::Duration) {
        let us = elapsed.as_micros() as u64;
        let prev = self.slowest_dir_us.load(Ordering::Relaxed);
        if us > prev {
            *self.slowest_path.lock().unwrap() = path.to_string();
        }
        self.slowest_dir_us.fetch_max(us, Ordering::Relaxed);
    }
}

/// A scope guard that adds its lifetime to one timing cell on drop.
pub(crate) struct Timed<'a> {
    timings: &'a ScanTimings,
    cell: &'a AtomicU64,
    started: Instant,
}

impl<'a> Timed<'a> {
    pub(crate) fn start(timings: &'a ScanTimings, cell: &'a AtomicU64) -> Self {
        Self {
            timings,
            cell,
            started: Instant::now(),
        }
    }
}

impl<'a> Drop for Timed<'a> {
    fn drop(&mut self) {
        self.timings.add(self.cell, self.started.elapsed());
    }
}

/// Whether the per-phase timing report was requested.
pub fn timings_enabled() -> bool {
    std::env::var("SIFT_SCAN_TIMINGS").is_ok_and(|value| value != "0" && !value.is_empty())
}
