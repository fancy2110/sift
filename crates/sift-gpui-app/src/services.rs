//! The shared core wiring: volumes, scanning, external deletions, analysis,
//! monitoring and deletion.
//!
//! Every one of these talks to a blocking core API, so every one of them runs
//! on a background thread and reports back over a single channel. The UI thread
//! only ever drains that channel, applies the result to [`WorkspaceModel`], and
//! redraws — it never performs I/O.
//!
//! ```text
//!   background threads ──Signal──► channel ──polled pump──► model ──► view
//! ```
//!
//! One channel rather than six pumps keeps the wiring readable and gives the
//! view a single place to reason about ordering.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sift_analyze::{
    AnalysisPolicy, AnalysisReport, Analyzer, CachedAdjudicator, PathFingerprint, RuleAdjudicator,
    Verdict, VerdictCache,
};
use sift_core::{NodeKey, ScanEvent, ScanId, ScanPolicy, ScanRequest, ScanTree, Volume, VolumeId};
use sift_monitor::{
    CleanupSource, Monitor, MonitorControl, MonitorEvent, SystemTrash,
};
use sift_platform::trash::TrashResult;
use sift_platform::watch::{FsEvent, FsWatcherHandle};
use sift_scan::{ScanControl, ScanEngine};
use sift_store::{MonitorSettings, CleanableEntry, CleanableList, Store};

use gpui_kit::{Context, Entity};

use crate::app::AppView;
use crate::model::{Surface, ToastLevel, WorkspaceModel};

/// How often the UI polls the background channel. Fast enough that a scan
/// feels live, slow enough that an idle window is not spinning.
const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Wall-clock milliseconds, the unit every core API stamps events with.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// One message from a background thread to the window.
enum Signal {
    Volumes(Vec<Volume>),
    Scan(ScanEvent),
    Fs(FsEvent),
    Monitor(MonitorEvent),
    Analysis(Result<AnalysisReport, String>),
    Cleanup(Vec<TrashResult>),
}

/// A running scan the view can still steer.
struct ScanSession {
    control: ScanControl,
    tree: Arc<Mutex<ScanTree>>,
}

impl ScanSession {
    fn cancel(&self) {
        self.control.cancel();
    }
}

/// A shared, delegating view of the durable store.
///
/// `sift_store::Store` owns a mutex and is not `Clone`, but the monitor needs an
/// `Arc<dyn CleanupSource>` for its whole lifetime while the analysis pipeline
/// needs a `VerdictCache`. This newtype hands the same store to both without
/// either of them owning it, and without adding a second source of truth.
#[derive(Clone)]
struct SharedStore(Arc<Store>);

impl VerdictCache for SharedStore {
    fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
        self.0.lookup_verdict(fingerprint, 0)
    }

    fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict) {
        self.0.store_verdict(fingerprint, verdict, verdict.judged_at_ms);
    }
}

impl CleanupSource for SharedStore {
    fn cleanable(&self) -> CleanableList {
        self.0.cleanable()
    }

    fn settings(&self) -> MonitorSettings {
        self.0.settings().monitor
    }

    fn note_removed(&self, entry: &CleanableEntry, now: i64) {
        self.0.note_removed(entry, now);
    }
}

/// Everything the window shares with the core.
pub struct Services {
    model: Entity<WorkspaceModel>,
    store: Arc<Store>,
    engine: ScanEngine,
    home: Option<PathBuf>,
    tx: mpsc::Sender<Signal>,

    volume: Option<Volume>,
    scan: Option<ScanSession>,
    watcher: Option<FsWatcherHandle>,
    monitor: Option<MonitorControl>,
    scan_seq: u64,
    analyzing: bool,
    cleaning: bool,
}

impl Services {
    /// Wire the background channel and start loading volumes.
    ///
    /// The store is opened by the caller before the window exists, so its file
    /// I/O never competes with a frame; any load warning is shown as a toast.
    pub fn new(
        model: Entity<WorkspaceModel>,
        store: Arc<Store>,
        warnings: Vec<String>,
        cx: &mut Context<AppView>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let services = Self {
            model: model.clone(),
            store,
            engine: ScanEngine::new(),
            home: dirs::home_dir(),
            tx: tx.clone(),
            volume: None,
            scan: None,
            watcher: None,
            monitor: None,
            scan_seq: 0,
            analyzing: false,
            cleaning: false,
        };

        Self::spawn_pump(rx, cx);

        // A damaged document is archived and defaults are used; the user is told
        // once, here, rather than silently losing their conclusions.
        let now = now_ms();
        for warning in warnings {
            model.update(cx, |model, cx| {
                model.toast(warning, ToastLevel::Warning, now);
                cx.notify();
            });
        }

        let volumes_tx = tx;
        let spawned = thread::Builder::new()
            .name("sift-volumes".into())
            .spawn(move || {
                let volumes = sift_platform::volume::list_volumes();
                let _ = volumes_tx.send(Signal::Volumes(volumes));
            });
        debug_assert!(spawned.is_ok(), "volume thread must start");

        services
    }

    /// Begin listening for background results.
    fn spawn_pump(rx: mpsc::Receiver<Signal>, cx: &mut Context<AppView>) {
        cx.spawn(async move |this, cx| {
            loop {
                let mut batch: Vec<Signal> = Vec::new();
                loop {
                    match rx.try_recv() {
                        Ok(signal) => batch.push(signal),
                        Err(mpsc::TryRecvError::Empty) => break,
                        // The channel cannot close while `Services` holds a
                        // sender; treat it as a stop signal if it ever does.
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                if !batch.is_empty()
                    && this
                        .update(cx, |view, cx| view.services_mut().apply(batch, cx))
                        .is_err()
                {
                    return;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    // ---- accessors ---------------------------------------------------------

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn volume(&self) -> Option<&Volume> {
        self.volume.as_ref()
    }

    pub fn is_monitoring(&self) -> bool {
        self.monitor.is_some()
    }

    // ---- signal handling ---------------------------------------------------

    /// Apply one batch of background results to the model.
    ///
    /// Kept as one function so ordering within a batch is explicit and a reader
    /// can see every path that touches the model.
    fn apply(&mut self, batch: Vec<Signal>, cx: &mut Context<AppView>) {
        for signal in batch {
            match signal {
                Signal::Volumes(volumes) => self.accept_volumes(volumes, cx),
                Signal::Scan(event) => {
                    self.model.update(cx, |model, cx| {
                        model.apply_scan_event(event);
                        cx.notify();
                    });
                }
                Signal::Fs(FsEvent::Deleted(path)) => {
                    let key = NodeKey::from_path(&path);
                    self.model.update(cx, |model, cx| {
                        if model.prune_subtree(key) {
                            cx.notify();
                        }
                    });
                }
                Signal::Fs(_) => {}
                Signal::Monitor(event) => {
                    let now = now_ms();
                    self.model.update(cx, |model, cx| {
                        model.apply_monitor_event(&event, now);
                        cx.notify();
                    });
                }
                Signal::Analysis(Ok(report)) => self.accept_analysis(&report, cx),
                Signal::Analysis(Err(message)) => {
                    let now = now_ms();
                    self.analyzing = false;
                    self.model.update(cx, |model, cx| {
                        model.fail_analysis(message, now);
                        cx.notify();
                    });
                }
                Signal::Cleanup(results) => self.accept_cleanup(results, cx),
            }
        }
    }

    fn accept_volumes(&mut self, volumes: Vec<Volume>, cx: &mut Context<AppView>) {
        if volumes.is_empty() {
            self.model.update(cx, |model, cx| {
                model.set_volumes(Vec::new());
                cx.notify();
            });
            self.toast("没有找到可扫描的磁盘", ToastLevel::Warning, cx);
            return;
        }

        // The choice itself belongs to the model; this only remembers the answer
        // so `restart_scan` knows which disk it was asked to walk.
        let chosen = self.model.update(cx, |model, cx| {
            let chosen = model.adopt_volumes(volumes);
            cx.notify();
            chosen
        });
        if self.volume.is_none() {
            self.volume = chosen;
        }
        // Deliberately no scan here. The design稿 starts one on launch; this
        // application scans when the user asks, so opening a window does not
        // begin a multi-minute walk of the whole disk on its own.
    }

    fn accept_analysis(&mut self, report: &AnalysisReport, cx: &mut Context<AppView>) {
        let now = now_ms();
        self.store.record_analysis(report, now);
        let _ = self.store.flush_if_dirty();
        self.analyzing = false;

        let approved: HashSet<NodeKey> = self
            .store
            .cleanable()
            .entries()
            .iter()
            .filter(|entry| entry.approved_for_auto)
            .map(|entry| entry.key())
            .collect();
        self.model.update(cx, |model, cx| {
            model.apply_analysis(report, &approved);
            cx.notify();
        });
    }

    fn accept_cleanup(&mut self, results: Vec<TrashResult>, cx: &mut Context<AppView>) {
        let now = now_ms();
        let cleanable = self.store.cleanable();
        let mut removed = 0usize;
        let mut failed = 0usize;
        let mut bytes = 0u64;
        let mut first_error: Option<String> = None;

        for result in &results {
            if !result.ok {
                failed += 1;
                if first_error.is_none() {
                    first_error = result.error.clone();
                }
                continue;
            }
            removed += 1;
            let key = NodeKey::from_path(&result.path);
            if let Some(entry) = cleanable.get(key) {
                bytes += entry.size;
                self.store.note_removed(entry, now);
            }
            self.model.update(cx, |model, cx| {
                model.prune_subtree(key);
                cx.notify();
            });
        }

        let _ = self.store.flush_if_dirty();
        self.cleaning = false;

        let message = if failed == 0 {
            format!(
                "已把 {removed} 项移入废纸篓，可回收 {}",
                sift_core::format_bytes(bytes)
            )
        } else {
            format!(
                "移入废纸篓 {removed} 项，{failed} 项失败：{}",
                first_error.unwrap_or_else(|| "未知原因".to_string())
            )
        };
        let level = if failed == 0 {
            ToastLevel::Success
        } else {
            ToastLevel::Warning
        };
        self.toast(message, level, cx);
    }

    fn toast(&mut self, message: impl Into<String>, level: ToastLevel, cx: &mut Context<AppView>) {
        let now = now_ms();
        self.model.update(cx, |model, cx| {
            model.toast(message, level, now);
            cx.notify();
        });
    }

    // ---- volumes and scanning ---------------------------------------------

    /// Switch to a volume and scan it.
    /// Re-list the mounted volumes now, on the caller's thread.
    ///
    /// The startup path enumerates on its own thread and delivers the list
    /// through the pump; this is the synchronous equivalent for a refresh the
    /// user asked for, and the entry point a test can drive without waiting on a
    /// background timer.
    pub fn refresh_volumes(&mut self, cx: &mut Context<AppView>) {
        let volumes = sift_platform::volume::list_volumes();
        self.accept_volumes(volumes, cx);
    }

    pub fn select_volume(&mut self, id: VolumeId, cx: &mut Context<AppView>) {
        let selected = self
            .model
            .read(cx)
            .volumes()
            .iter()
            .find(|volume| volume.id == id)
            .cloned();
        let Some(volume) = selected else {
            return;
        };
        self.volume = Some(volume);
        self.model.update(cx, |model, cx| {
            model.select_volume(id);
            cx.notify();
        });
        // Choosing a disk is not the same as asking for a walk of it: the scan
        // control is the one way to start one, so a user who only wants to look
        // at another disk is not charged a multi-minute scan for it.
    }

    /// Show every mounted volume as one summary surface.
    pub fn show_all_volumes(&mut self, cx: &mut Context<AppView>) {
        self.stop_scan();
        self.model.update(cx, |model, cx| {
            model.show_all_volumes();
            cx.notify();
        });
    }

    /// Scan the current volume from its root.
    pub fn restart_scan(&mut self, cx: &mut Context<AppView>) {
        self.stop_scan();
        let Some(volume) = self.volume.clone() else {
            return;
        };
        // The synthetic "all volumes" surface has no single root to walk.
        if self.model.read(cx).surface() == Some(Surface::AllVolumes) {
            return;
        }

        self.scan_seq += 1;
        let root = volume.mount_point.clone();
        let request = ScanRequest::new(ScanId(self.scan_seq), root.clone())
            .with_focus(root.clone())
            .with_policy(ScanPolicy::thorough())
            .with_volume_used_bytes(volume.used_bytes());

        self.model.update(cx, |model, cx| {
            model.begin_scan();
            cx.notify();
        });

        let handle = match self.engine.scan(request) {
            Ok(handle) => handle,
            Err(error) => {
                let message = format!("无法扫描 {}：{error}", volume.mount_point.display());
                self.toast(message, ToastLevel::Error, cx);
                return;
            }
        };

        let scan_id = handle.scan_id;
        let tree = Arc::clone(&handle.tree);
        let control = handle.control();
        self.watch_root(root);

        let events = handle.events;
        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
            .name("sift-scan-bridge".into())
            .spawn(move || {
                // Events from a superseded scan are dropped here rather than
                // being allowed to resurrect a cleared tree.
                while let Ok(event) = events.recv() {
                    if event.scan_id() != scan_id {
                        continue;
                    }
                    if tx.send(Signal::Scan(event)).is_err() {
                        break;
                    }
                }
            });
        debug_assert!(spawned.is_ok(), "scan bridge thread must start");

        self.scan = Some(ScanSession { control, tree });
    }

    /// Re-focus a running scan after the user drills into a directory.
    pub fn focus_scan(&self, path: PathBuf) {
        if let Some(session) = self.scan.as_ref() {
            session.control.set_focus(path);
        }
    }

    fn stop_scan(&mut self) {
        if let Some(session) = self.scan.take() {
            session.cancel();
        }
        self.watcher = None;
    }

    fn watch_root(&mut self, root: PathBuf) {
        self.watcher = None;
        let Ok((handle, events)) = FsWatcherHandle::watch(root) else {
            return;
        };
        self.watcher = Some(handle);

        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
            .name("sift-fs-bridge".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if tx.send(Signal::Fs(event)).is_err() {
                        break;
                    }
                }
            });
        debug_assert!(spawned.is_ok(), "filesystem bridge thread must start");
    }

    // ---- analysis ----------------------------------------------------------

    pub fn run_analysis(&mut self, cx: &mut Context<AppView>) {
        if self.analyzing {
            return;
        }
        let Some(session) = self.scan.as_ref() else {
            self.toast("先扫描磁盘，再运行分析", ToastLevel::Info, cx);
            return;
        };
        if self.model.read(cx).current_dir().is_none() {
            self.toast("扫描完成后再运行分析", ToastLevel::Info, cx);
            return;
        }

        self.analyzing = true;
        self.model.update(cx, |model, cx| {
            model.begin_analysis();
            cx.notify();
        });

        let tree = Arc::clone(&session.tree);
        let analyzer = Analyzer::new(AnalysisPolicy::default(), self.home.clone());
        let store = SharedStore(Arc::clone(&self.store));
        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
            .name("sift-analyze".into())
            .spawn(move || {
                // Remote adjudication is feature-gated in the Tauri front end
                // (`sift-analyze/remote-ai`). This front end deliberately has no
                // network client, so the local rules remain the only adjudicator
                // even when the stored AI settings report themselves usable.
                let adjudicator = CachedAdjudicator::new(RuleAdjudicator::new(), store);
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut tree = tree.lock().expect("scan tree lock");
                    analyzer.analyze(&adjudicator, &mut tree, now_ms(), |_, _, _| {})
                }))
                .map_err(|_| "分析过程出错，已保留上次结论".to_string());
                let _ = tx.send(Signal::Analysis(outcome));
            });
        debug_assert!(spawned.is_ok(), "analysis thread must start");
    }

    /// Remember (or revoke) unattended-cleanup approval for one finding.
    pub fn set_auto_approval(&mut self, key: NodeKey, approved: bool, cx: &mut Context<AppView>) {
        self.store.set_auto_approval(key, approved);
        let _ = self.store.flush_if_dirty();
        self.model.update(cx, |model, cx| {
            model.mark_approved(key, approved);
            cx.notify();
        });
    }

    // ---- monitoring --------------------------------------------------------

    pub fn set_monitor_running(&mut self, running: bool, cx: &mut Context<AppView>) {
        if !running {
            if let Some(control) = self.monitor.take() {
                control.cancel();
            }
            self.model.update(cx, |model, cx| {
                model.set_monitor_running(false);
                cx.notify();
            });
            return;
        }

        if self.monitor.is_some() {
            return;
        }
        let Some(volume) = self.volume.clone() else {
            self.toast("先选择一个磁盘，再开启自动清理", ToastLevel::Info, cx);
            return;
        };

        let source: Arc<dyn CleanupSource> = Arc::new(SharedStore(Arc::clone(&self.store)));
        let started = Monitor::start_with_remover(
            volume,
            source,
            self.home.clone(),
            Arc::new(SystemTrash),
        );

        match started {
            Ok(handle) => {
                let (events, control) = handle.into_parts();
                self.monitor = Some(control);
                self.model.update(cx, |model, cx| {
                    model.set_monitor_running(true);
                    cx.notify();
                });
                let tx = self.tx.clone();
                let spawned = thread::Builder::new()
                    .name("sift-monitor-bridge".into())
                    .spawn(move || {
                        while let Ok(event) = events.recv() {
                            if tx.send(Signal::Monitor(event)).is_err() {
                                break;
                            }
                        }
                    });
                debug_assert!(spawned.is_ok(), "monitor bridge thread must start");
            }
            Err(error) => {
                let message = format!("无法启动后台监控：{error}");
                self.toast(message, ToastLevel::Error, cx);
            }
        }
    }

    /// Take a monitor reading now, so the user does not wait for the interval.
    pub fn sample_monitor_now(&self) {
        if let Some(control) = self.monitor.as_ref() {
            control.sample_now();
        }
    }

    // ---- deletion ----------------------------------------------------------

    /// Send the current selection to the platform trash.
    ///
    /// Confirmation has already happened in the UI; this performs the move and
    /// records the outcome, so nothing is ever erased.
    pub fn trash_selection(&mut self, cx: &mut Context<AppView>) {
        if self.cleaning {
            return;
        }
        let paths: Vec<PathBuf> = self
            .model
            .read(cx)
            .selected_items()
            .iter()
            .map(|node| node.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }

        self.cleaning = true;
        self.model.update(cx, |model, cx| {
            model.set_drawer_open(false);
            cx.notify();
        });

        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
            .name("sift-trash".into())
            .spawn(move || {
                let results = sift_platform::trash::trash_paths(&paths);
                let _ = tx.send(Signal::Cleanup(results));
            });
        debug_assert!(spawned.is_ok(), "trash thread must start");
    }
}
