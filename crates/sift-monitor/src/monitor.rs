//! The background monitor: a long-lived thread that watches free space.
//!
//! It is deliberately thin. All the judgement is in [`crate::policy::evaluate`];
//! this module only samples the volume, asks the policy, and carries out the
//! answer. That split is what makes the safety rules reviewable — there is no
//! second place where a deletion could be authorized.
//!
//! Every removal goes through the platform trash, is preceded by a fresh
//! fingerprint check, and is reported as an event whether it succeeded or not.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use sift_core::Volume;
use sift_store::{CleanableEntry, CleanableList, MonitorSettings, Store};

use crate::policy::{
    evaluate, still_matches, AlertLevel, ConfirmationReason, DiskSample, MonitorAction,
    MonitorConfig, MonitorDecision,
};

/// Where the monitor reads conclusions from and reports removals to.
///
/// A narrow trait rather than a concrete [`Store`] so the monitor can be driven
/// by a test double, and so the store never has to know a watcher exists.
pub trait CleanupSource: Send + Sync {
    /// The entries the product has concluded are definitively cleanable.
    fn cleanable(&self) -> CleanableList;
    /// Current monitor preferences.
    fn settings(&self) -> MonitorSettings;
    /// Report a successful removal so it leaves the list and enters the
    /// decision log.
    fn note_removed(&self, entry: &CleanableEntry, now_ms: i64);

    /// Record one finished cleanup session (a set of removals as a single
    /// history row). A default no-op keeps test doubles minimal.
    fn note_cleanup_session(&self, titles: &[String], bytes: u64, automatic: bool, now_ms: i64) {
        let _ = (titles, bytes, automatic, now_ms);
    }
}

impl CleanupSource for Store {
    fn cleanable(&self) -> CleanableList {
        Store::cleanable(self)
    }

    fn settings(&self) -> MonitorSettings {
        Store::settings(self).monitor
    }

    fn note_removed(&self, entry: &CleanableEntry, now_ms: i64) {
        Store::note_removed(self, entry, now_ms);
    }

    fn note_cleanup_session(&self, titles: &[String], bytes: u64, automatic: bool, now_ms: i64) {
        Store::note_cleanup_session(self, titles, bytes, automatic, now_ms);
    }
}

/// How entries are actually made to disappear.
///
/// A seam rather than a direct call to the trash so the cleanup path — the one
/// place that deletes without a user action in front of it — can be exercised in
/// tests without depending on the platform's trash permission. The shipped
/// implementation is [`SystemTrash`], which always moves to the OS trash and
/// never erases.
pub trait Remover: Send + Sync {
    fn remove(&self, paths: &[std::path::PathBuf]) -> Vec<sift_platform::trash::TrashResult>;
}

/// The production remover: the operating system's trash.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTrash;

impl Remover for SystemTrash {
    fn remove(&self, paths: &[std::path::PathBuf]) -> Vec<sift_platform::trash::TrashResult> {
        sift_platform::trash::trash_paths(paths)
    }
}

/// What the monitor tells the front end.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MonitorEvent {
    /// A routine reading, for the status area.
    Sampled {
        level: AlertLevel,
        sample: DiskSample,
        reclaimable_bytes: u64,
        entry_count: usize,
    },
    /// The threshold was crossed and the user should be told.
    Notify {
        level: AlertLevel,
        available_bytes: u64,
        reclaimable_bytes: u64,
        entry_count: usize,
    },
    /// Items could be cleaned but need the user's go-ahead.
    ConfirmationNeeded {
        level: AlertLevel,
        reason: ConfirmationReason,
        reclaimable_bytes: u64,
        entry_count: usize,
    },
    /// An unattended cleanup is starting.
    CleanupStarted { entry_count: usize, bytes: u64 },
    /// An unattended cleanup finished, with per-outcome counts.
    CleanupFinished {
        removed: usize,
        skipped: usize,
        failed: usize,
        bytes: u64,
        paths: Vec<PathBuf>,
    },
    /// A non-fatal problem worth surfacing once (volume vanished, no home dir).
    Warning(String),
}

/// Control handle for a running monitor.
pub struct MonitorHandle {
    pub events: mpsc::Receiver<MonitorEvent>,
    cancel: Arc<AtomicBool>,
    /// Ask for an immediate sample instead of waiting for the interval.
    wake: mpsc::Sender<()>,
    finished: Arc<AtomicBool>,
}

/// A cloneable control handle for a running monitor.
///
/// The event receiver is single-consumer, so a supervisor (an IPC layer, a UI
/// state object) needs to own the stream and keep only this control.
#[derive(Clone)]
pub struct MonitorControl {
    cancel: Arc<AtomicBool>,
    wake: mpsc::Sender<()>,
    finished: Arc<AtomicBool>,
}

impl MonitorControl {
    /// Stop the monitor thread soon.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
    }

    /// Take a reading now.
    pub fn sample_now(&self) {
        let _ = self.wake.send(());
    }

    /// Whether the monitor thread is still running.
    pub fn is_running(&self) -> bool {
        !self.finished.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for MonitorControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorControl")
            .field("running", &self.is_running())
            .finish()
    }
}

impl MonitorHandle {
    /// A cloneable control, for a supervisor that takes the event stream.
    pub fn control(&self) -> MonitorControl {
        MonitorControl {
            cancel: Arc::clone(&self.cancel),
            wake: self.wake.clone(),
            finished: Arc::clone(&self.finished),
        }
    }

    /// Split into the event stream and a control handle.
    ///
    /// The control keeps the monitor alive (it holds the wake channel), so
    /// moving the receiver into a pump thread does not stop the watch.
    pub fn into_parts(self) -> (mpsc::Receiver<MonitorEvent>, MonitorControl) {
        let control = self.control();
        (self.events, control)
    }

    /// Stop the monitor thread soon.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
    }

    /// Take a reading now. Used by the UI's "check now" affordance.
    pub fn sample_now(&self) {
        let _ = self.wake.send(());
    }

    pub fn is_running(&self) -> bool {
        !self.finished.load(Ordering::SeqCst)
    }

    /// Drain events without blocking; convenient for a UI render pass.
    pub fn drain(&self) -> Vec<MonitorEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

/// Long-running free-space watcher.
pub struct Monitor;

impl Monitor {
    /// Start watching `volume` on a background thread.
    ///
    /// `home` is the boundary for unattended action; without it the policy
    /// refuses to select anything.
    pub fn start(
        volume: Volume,
        source: Arc<dyn CleanupSource>,
        home: Option<PathBuf>,
    ) -> std::io::Result<MonitorHandle> {
        Self::start_with_remover(volume, source, home, Arc::new(SystemTrash))
    }

    /// Start watching with an injected [`Remover`], for tests and for a front end
    /// that wants to mediate removals itself.
    pub fn start_with_remover(
        volume: Volume,
        source: Arc<dyn CleanupSource>,
        home: Option<PathBuf>,
        remover: Arc<dyn Remover>,
    ) -> std::io::Result<MonitorHandle> {
        let (event_tx, event_rx) = mpsc::channel::<MonitorEvent>();
        let (wake_tx, wake_rx) = mpsc::channel::<()>();
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));

        let thread_cancel = Arc::clone(&cancel);
        let thread_finished = Arc::clone(&finished);
        std::thread::Builder::new()
            .name("sift-monitor".into())
            .spawn(move || {
                monitor_loop(
                    volume,
                    source,
                    home,
                    remover,
                    event_tx,
                    wake_rx,
                    thread_cancel,
                );
                thread_finished.store(true, Ordering::SeqCst);
            })?;

        Ok(MonitorHandle {
            events: event_rx,
            cancel,
            wake: wake_tx,
            finished,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn monitor_loop(
    volume: Volume,
    source: Arc<dyn CleanupSource>,
    home: Option<PathBuf>,
    remover: Arc<dyn Remover>,
    event_tx: mpsc::Sender<MonitorEvent>,
    wake_rx: mpsc::Receiver<()>,
    cancel: Arc<AtomicBool>,
) {
    let mut last_clean_ms: Option<i64> = None;
    let mut last_notify_ms: Option<i64> = None;

    if home.is_none() {
        let _ = event_tx.send(MonitorEvent::Warning("err.monitorNoHome".to_string()));
    }

    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }

        let settings = source.settings();
        let config = MonitorConfig::from_settings(&settings);
        let now_ms = now_ms();

        match read_sample(&volume) {
            Some(sample) => {
                let entries = source.cleanable().entries().to_vec();
                let decision = evaluate(
                    &config,
                    &sample,
                    &entries,
                    home.as_deref(),
                    last_clean_ms,
                    last_notify_ms,
                    now_ms,
                );
                act(
                    &decision,
                    &entries,
                    &config,
                    home.as_deref(),
                    &source,
                    &remover,
                    &event_tx,
                    &mut last_clean_ms,
                    &mut last_notify_ms,
                    now_ms,
                );
            }
            None => {
                let _ = event_tx.send(MonitorEvent::Warning("err.monitorUnreadable".to_string()));
            }
        }

        // Sleep the interval, but wake early for cancellation or a manual
        // sample. `recv_timeout` gives both without a second timer thread.
        let interval = Duration::from_secs(settings.interval_secs.clamp(30, 24 * 3600));
        let deadline = std::time::Instant::now() + interval;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match wake_rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn act(
    decision: &MonitorDecision,
    entries: &[CleanableEntry],
    config: &MonitorConfig,
    home: Option<&Path>,
    source: &Arc<dyn CleanupSource>,
    remover: &Arc<dyn Remover>,
    event_tx: &mpsc::Sender<MonitorEvent>,
    last_clean_ms: &mut Option<i64>,
    last_notify_ms: &mut Option<i64>,
    now_ms: i64,
) {
    let entry_count = decision.selected().len();
    match &decision.action {
        MonitorAction::Quiet => {
            let _ = event_tx.send(MonitorEvent::Sampled {
                level: decision.level,
                sample: decision.sample,
                reclaimable_bytes: decision.reclaimable_bytes,
                entry_count: entries.len(),
            });
        }
        MonitorAction::Notify => {
            *last_notify_ms = Some(now_ms);
            let _ = event_tx.send(MonitorEvent::Notify {
                level: decision.level,
                available_bytes: decision.sample.available_bytes,
                reclaimable_bytes: decision.reclaimable_bytes,
                entry_count,
            });
        }
        MonitorAction::NeedsConfirmation { reason, .. } => {
            *last_notify_ms = Some(now_ms);
            let _ = event_tx.send(MonitorEvent::ConfirmationNeeded {
                level: decision.level,
                reason: *reason,
                reclaimable_bytes: decision.reclaimable_bytes,
                entry_count,
            });
        }
        MonitorAction::AutoClean { entries: selected } => {
            let _ = event_tx.send(MonitorEvent::CleanupStarted {
                entry_count: selected.len(),
                bytes: decision.reclaimable_bytes,
            });
            let outcome = clean_now(
                selected,
                home,
                config.home_only,
                &**source,
                &**remover,
                now_ms,
            );
            *last_clean_ms = Some(now_ms);
            let _ = event_tx.send(MonitorEvent::CleanupFinished {
                removed: outcome.removed,
                skipped: outcome.skipped,
                failed: outcome.failed,
                bytes: outcome.bytes,
                paths: outcome.removed_paths,
            });
        }
    }
}

/// Counts from one unattended cleanup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanupOutcome {
    pub removed: usize,
    /// Refused because the entry no longer matched its approved fingerprint, or
    /// because it fell outside the home boundary after all.
    pub skipped: usize,
    /// The trash reported a per-item failure.
    pub failed: usize,
    pub bytes: u64,
    pub removed_paths: Vec<PathBuf>,
}

/// Perform an unattended cleanup, verifying every entry first.
///
/// This is the only place in the product that deletes without a user action in
/// front of it, so it re-checks each entry against the filesystem immediately
/// before the move: an approval describes one object, and if the object changed
/// the approval no longer applies.
pub fn clean_now(
    selected: &[CleanableEntry],
    home: Option<&Path>,
    home_only: bool,
    source: &dyn CleanupSource,
    remover: &dyn Remover,
    now_ms: i64,
) -> CleanupOutcome {
    let mut outcome = CleanupOutcome::default();
    let mut verified: Vec<&CleanableEntry> = Vec::with_capacity(selected.len());

    for entry in selected {
        if home_only && !crate::policy::inside_home(&entry.path, home) {
            outcome.skipped += 1;
            continue;
        }
        if !still_matches(entry) {
            outcome.skipped += 1;
            continue;
        }
        verified.push(entry);
    }

    if verified.is_empty() {
        return outcome;
    }

    let paths: Vec<PathBuf> = verified.iter().map(|entry| entry.path.clone()).collect();
    let results = remover.remove(&paths);

    let mut session_titles: Vec<String> = Vec::new();
    for (entry, result) in verified.iter().zip(results.iter()) {
        if result.ok {
            outcome.removed += 1;
            outcome.bytes = outcome.bytes.saturating_add(entry.size);
            outcome.removed_paths.push(entry.path.clone());
            source.note_removed(entry, now_ms);
            session_titles.push(entry.name.clone());
        } else {
            outcome.failed += 1;
        }
    }
    if !session_titles.is_empty() {
        source.note_cleanup_session(&session_titles, outcome.bytes, true, now_ms);
    }
    outcome
}

/// Read the volume's current capacity.
fn read_sample(volume: &Volume) -> Option<DiskSample> {
    let (total, available) = sift_platform::volume::free_space(&volume.mount_point)?;
    Some(DiskSample::new(total, available, now_ms()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::{PathFingerprint, Reason, Safety, VerdictSource};
    use sift_core::NodeKey;
    use sift_core::VolumeId;
    use sift_store::AutoCleanMode;
    use std::sync::Mutex;

    /// A source that hands out a fixed list and records removals.
    struct FakeSource {
        entries: CleanableList,
        settings: Mutex<MonitorSettings>,
        removed: Mutex<Vec<String>>,
    }

    impl FakeSource {
        fn new(entries: CleanableList, settings: MonitorSettings) -> Self {
            Self {
                entries,
                settings: Mutex::new(settings),
                removed: Mutex::new(Vec::new()),
            }
        }
    }

    impl CleanupSource for FakeSource {
        fn cleanable(&self) -> CleanableList {
            self.entries.clone()
        }
        fn settings(&self) -> MonitorSettings {
            self.settings.lock().unwrap().clone()
        }
        fn note_removed(&self, entry: &CleanableEntry, _now_ms: i64) {
            self.removed.lock().unwrap().push(entry.name.clone());
        }
    }

    fn fake_entry(name: &str, path: PathBuf, size: u64, approved: bool) -> CleanableEntry {
        // Fingerprint the real object so `still_matches` can succeed.
        let meta = std::fs::symlink_metadata(&path).expect("fixture exists");
        CleanableEntry {
            fingerprint: PathFingerprint::new(
                NodeKey::from_path(&path),
                meta.len(),
                crate::policy::modified_ms(&meta),
                true,
            ),
            path: path.clone(),
            display_path: format!("~/{name}"),
            name: name.to_string(),
            kind_token: "rebuildableCache".into(),
            size,
            safety: Safety::Safe,
            confidence: 0.9,
            reason: Reason::key("k"),
            source: VerdictSource::rule("dir.node_modules"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: approved,
        }
    }

    fn volume_at(mount: &Path) -> Volume {
        Volume::new(
            VolumeId::from_mount_point(mount),
            "Test",
            mount,
            1_000_000_000_000,
            500_000_000_000,
            false,
            "apfs",
        )
    }

    #[test]
    fn a_healthy_volume_produces_a_sample_event_and_no_deletion() {
        let base = std::env::temp_dir();
        let source = Arc::new(FakeSource::new(
            CleanableList::default(),
            MonitorSettings::default(),
        ));
        let handle = Monitor::start(volume_at(&base), source.clone(), Some(base.clone())).unwrap();
        handle.sample_now();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_sample = false;
        while std::time::Instant::now() < deadline {
            if let Ok(event) = handle.events.recv_timeout(Duration::from_millis(200)) {
                if matches!(event, MonitorEvent::Sampled { .. }) {
                    saw_sample = true;
                    break;
                }
            }
        }
        handle.cancel();
        assert!(saw_sample, "a routine reading must be reported");
        assert!(source.removed.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unapproved_entry_is_never_removed() {
        let base = std::env::temp_dir().join(format!("sift-monitor-act-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("cache")).unwrap();

        let entry = fake_entry("cache", base.join("cache"), 100, false);
        let list = CleanableList::from_entries(vec![entry], 64);

        let settings = MonitorSettings {
            auto_mode: AutoCleanMode::AutoApproved,
            warn_free_ratio: 1.0,
            critical_free_ratio: 0.99,
            ..MonitorSettings::default()
        };
        let source = Arc::new(FakeSource::new(list, settings));
        let config = MonitorConfig::from_settings(&source.settings());
        let sample = DiskSample::new(1000, 10, 0);
        let entries = source.cleanable().entries().to_vec();
        let decision = evaluate(&config, &sample, &entries, Some(&base), None, None, 0);

        // The policy refuses to act on an unapproved entry even at 1% free.
        assert!(!decision.deletes());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn execute_cleanup_skips_a_changed_object_and_reports_it() {
        let base = std::env::temp_dir().join(format!("sift-monitor-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();

        let mut entry = fake_entry("cache", cache.clone(), 100, true);
        // Pretend the approval was given before the directory last changed.
        entry.fingerprint.mtime_ms = entry.fingerprint.mtime_ms.saturating_sub(5_000);

        let source: Arc<dyn CleanupSource> = Arc::new(FakeSource::new(
            CleanableList::default(),
            MonitorSettings::default(),
        ));
        let outcome = clean_now(&[entry], Some(&base), true, &*source, &SystemTrash, 0);

        assert_eq!(outcome.removed, 0);
        assert_eq!(outcome.skipped, 1, "a changed object must be skipped");
        assert_eq!(outcome.failed, 0);
        assert!(cache.exists(), "the fixture must survive");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn execute_cleanup_refuses_paths_outside_home() {
        let base = std::env::temp_dir().join(format!("sift-monitor-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();

        let entry = fake_entry("cache", cache.clone(), 100, true);
        let source: Arc<dyn CleanupSource> = Arc::new(FakeSource::new(
            CleanableList::default(),
            MonitorSettings::default(),
        ));
        // A home that does not contain the entry.
        let outcome = clean_now(
            std::slice::from_ref(&entry),
            Some(Path::new("/nonexistent-home")),
            true,
            &*source,
            &SystemTrash,
            0,
        );
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.removed, 0);
        assert!(cache.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_handle_reports_running_state_and_drains() {
        let base = std::env::temp_dir();
        let source = Arc::new(FakeSource::new(
            CleanableList::default(),
            MonitorSettings::default(),
        ));
        let handle = Monitor::start(volume_at(&base), source, Some(base.clone())).unwrap();
        assert!(handle.is_running());
        let _ = handle.drain();
        handle.cancel();
        // Give the thread a moment to observe the flag.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while handle.is_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!handle.is_running(), "cancel must stop the thread");
    }
}
