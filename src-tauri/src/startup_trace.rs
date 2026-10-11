//! Startup performance tracing.
//!
//! Collects named phase marks from process entry through the first meaningful
//! webview paint, so cold-start regressions are measurable instead of guessed.
//! Marks are always cheap to record (a monotonic timestamp push); the report is
//! printed only when `SIFT_STARTUP_TRACE=1` is set, and it can also be fetched
//! by the `startup_trace` command for automated checks.

use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

struct StartupTrace {
    /// Monotonic anchor captured at the first line of `main` (misses dyld
    /// pre-main time, which the external wall-clock mark covers).
    origin: Instant,
    /// Wall-clock equivalent of `origin`, so webview marks (which only know
    /// `Date.now()`) line up with process phases on the same machine clock.
    origin_wall_ms: i64,
    marks: Mutex<Vec<Mark>>,
}

#[derive(Clone)]
pub struct Mark {
    pub label: String,
    /// Milliseconds since the process anchor.
    pub since_origin_ms: u64,
    /// Wall-clock milliseconds reported by the webview (`Date.now()`); `None`
    /// for native marks.
    pub wall_ms: Option<i64>,
}

static TRACE: OnceLock<StartupTrace> = OnceLock::new();

fn wall_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Initialise the trace. Called once from `main`.
pub fn begin() {
    let origin_wall_ms = wall_now_ms();
    if std::env::var("SIFT_STARTUP_TRACE").as_deref() == Ok("1") {
        // Lets an external launcher subtract its own spawn timestamp to get
        // the dyld/pre-main cost that the in-process anchor cannot see.
        eprintln!("[startup] origin-wall-ms={origin_wall_ms}");
    }
    let _ = TRACE.set(StartupTrace {
        origin: Instant::now(),
        origin_wall_ms,
        marks: Mutex::new(Vec::new()),
    });
}

fn enabled() -> bool {
    std::env::var("SIFT_STARTUP_TRACE").as_deref() == Ok("1")
}

fn trace() -> Option<&'static StartupTrace> {
    TRACE.get()
}

/// Record a native startup phase.
pub fn mark(label: &str) {
    let Some(trace) = trace() else { return };
    let since = trace.origin.elapsed().as_millis() as u64;
    if enabled() {
        eprintln!("[startup] +{since:>5} ms  {label}");
    }
    trace.marks.lock().unwrap().push(Mark {
        label: label.to_string(),
        since_origin_ms: since,
        wall_ms: None,
    });
}

/// Record a mark timestamped by the webview's wall clock. The end-to-end time
/// (process entry to first paint) is the difference from the native anchor.
pub fn mark_from_webview(label: &str, wall_ms: i64) {
    let Some(trace) = trace() else { return };
    let since = wall_ms.saturating_sub(trace.origin_wall_ms).max(0) as u64;
    if enabled() {
        eprintln!("[startup] +{since:>5} ms  {label} (webview)");
    }
    trace.marks.lock().unwrap().push(Mark {
        label: label.to_string(),
        since_origin_ms: since,
        wall_ms: Some(wall_ms),
    });
}

/// Snapshot of every mark, for the `startup_trace` command.
pub fn marks() -> Vec<Mark> {
    trace()
        .map(|trace| trace.marks.lock().unwrap().clone())
        .unwrap_or_default()
}
