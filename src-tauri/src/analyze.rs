//! Tauri commands for the analysis, persistence and monitoring layers.
//!
//! This is where the three product requirements meet the Tauri front end:
//!
//! 1. [`analyze_current`] judges the scanned tree — rebuildable caches, stale
//!    files, installers, duplicates — and returns a safety level, a confidence,
//!    and a reason for each candidate. When a remote provider is configured and
//!    consented to, the content-dependent candidates are reviewed by it through
//!    the same guardrails the native front end uses.
//! 2. The conclusions are written to the local store, so the definitively
//!    cleanable list survives a restart and feeds habit mining.
//! 3. [`monitor_status`] / [`start_monitor`] / [`stop_monitor`] expose the
//!    background watch whose policy can clean approved items unattended.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sift_analyze::cleanup::{display_command, plan_for};
use sift_analyze::{
    redact_path, Adjudicator, AnalysisPolicy, AnalysisStage, Analyzer, Decision, DecisionOutcome,
    Reason, RuleAdjudicator, Safety, VerdictSource,
};
use sift_monitor::{CleanupSource, Monitor, MonitorEvent};
use sift_store::{AutoCleanMode, Store, StoreWarning};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::scanner::ScanManager;

/// What the front end needs to know about one item from the last analysis.
#[derive(Debug, Clone)]
struct ReportItem {
    key: sift_core::NodeKey,
    name: String,
    display_path: String,
    is_dir: bool,
    size: u64,
    mtime_ms: i64,
    kind_token: String,
    safety: Safety,
}

/// Services the analysis commands need: durable state and the running monitor.
pub struct AppServices {
    pub store: Arc<Store>,
    pub monitor: Mutex<Option<sift_monitor::MonitorControl>>,
    /// Warnings collected while loading the store, drained by the front end.
    pub warnings: Mutex<Vec<StoreWarning>>,
    /// Items from the most recent analysis keyed by real path, so a cleanup can
    /// tell an explicitly confirmed review item from an unknown path.
    last_report: Mutex<HashMap<PathBuf, ReportItem>>,
}

impl AppServices {
    pub fn new() -> Self {
        let (store, warnings) = Store::open_default();
        Self {
            store: Arc::new(store),
            monitor: Mutex::new(None),
            warnings: Mutex::new(warnings),
            last_report: Mutex::new(HashMap::new()),
        }
    }

    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    pub fn monitor_running(&self) -> bool {
        self.monitor
            .lock()
            .unwrap()
            .as_ref()
            .map(|handle| handle.is_running())
            .unwrap_or(false)
    }
}

impl Default for AppServices {
    fn default() -> Self {
        Self::new()
    }
}

// ---- DTOs ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub display_path: String,
    pub size: u64,
    pub is_dir: bool,
    /// `safe` | `review` | `keep`.
    pub safety: String,
    pub confidence: f32,
    /// For a local verdict this is a translation key; for a model's verdict it
    /// is ready-to-display text. `reason_kind` says which.
    pub reason: String,
    pub reason_kind: String,
    pub reason_params: Vec<String>,
    pub source: String,
    pub kind: String,
    /// Remembered as definitively cleanable.
    pub known_cleanable: bool,
    /// Approved for unattended cleanup.
    pub approved_for_auto: bool,
    /// Toolchain-native command preferred for cleanup, when one exists.
    pub cleanup_command: Option<String>,
    /// How the item is cleaned: nativeCommand | trashItem | emptyTrash.
    pub cleanup_method: String,
    /// Deletion impact: translation key for local verdicts, ready text for a
    /// model's verdict.
    pub impact: String,
    pub impact_kind: String,
    pub impact_params: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisSummaryDto {
    pub findings: Vec<FindingDto>,
    pub reclaimable_bytes: u64,
    pub safe_bytes: u64,
    pub known_cleanable_bytes: u64,
    pub source_counts: BTreeMap<String, usize>,
    /// Whether a remote adjudicator took part.
    pub used_remote: bool,
    /// A provider that is configured but not consented to, so the UI can ask.
    pub remote_needs_consent: bool,
}

/// AI provider configuration for the settings sheet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigDto {
    pub enabled: bool,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub language: String,
    pub batch_size: usize,
    /// Whether a token is stored (Keychain or the `SIFT_AI_API_KEY`
    /// environment variable); the secret itself is never returned.
    pub has_token: bool,
}

/// Settings-sheet submission. `token` semantics: `None` keeps the stored
/// secret, `Some("")` clears it, `Some(value)` replaces it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAiConfigInput {
    pub enabled: bool,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStatusDto {
    pub running: bool,
    pub enabled: bool,
    pub auto_mode: String,
    pub cleanable_items: usize,
    pub auto_eligible_items: usize,
    pub auto_eligible_bytes: u64,
    pub last_level: String,
    pub last_available_bytes: u64,
    pub last_total_bytes: u64,
    pub scheduled_cleanup_enabled: bool,
    pub scheduled_hour: u32,
}

// ---- commands --------------------------------------------------------------

/// Analyze the most recent scan and persist the conclusions.
#[tauri::command]
pub fn analyze_current(
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> Result<AnalysisSummaryDto, String> {
    analyze_current_with(&services, &manager)
}

/// Analyse the last scan with concrete services, callable from non-command code.
pub(crate) fn analyze_current_with(
    services: &AppServices,
    manager: &ScanManager,
) -> Result<AnalysisSummaryDto, String> {
    let tree = manager
        .last_tree()
        .ok_or_else(|| "err.noScanYet".to_string())?;

    let settings = services.store.settings();
    let policy = AnalysisPolicy {
        min_candidate_bytes: settings.analysis.min_candidate_bytes,
        include_files: settings.analysis.include_files,
        ..AnalysisPolicy::default()
    }
    .with_min_bytes(settings.analysis.min_candidate_bytes);

    let analyzer = Analyzer::new(policy, dirs_home());
    let adjudicator = build_adjudicator(services);
    let used_remote = adjudicator.is_remote();

    let report = {
        let mut guard = tree.lock().map_err(|err| err.to_string())?;
        analyzer.analyze(
            &*adjudicator,
            &mut guard,
            now_ms(),
            |_stage, _done, _total| {},
        )
    };

    // Requirement 2: persist what is definitively cleanable plus the verdicts.
    services.store.record_analysis(&report, now_ms());
    let _ = services.store.flush_if_dirty();

    let cleanable = services.store.cleanable();
    {
        // Remember every analyzed item by its real path so clean_paths can
        // distinguish a confirmed review item from an unknown path.
        let mut last_report = services.last_report.lock().unwrap();
        last_report.clear();
        for item in &report.items {
            last_report.insert(
                item.candidate.path.clone(),
                ReportItem {
                    key: item.candidate.key,
                    name: item.candidate.name.clone(),
                    display_path: item.candidate.display_path.clone(),
                    is_dir: item.candidate.is_dir,
                    size: item.candidate.reclaimable(),
                    mtime_ms: item.candidate.mtime_ms,
                    kind_token: item.candidate.kind.token().to_string(),
                    safety: item.verdict.safety,
                },
            );
        }
    }
    let findings: Vec<FindingDto> = report
        .sorted_by_size()
        .into_iter()
        .map(|item| finding_dto(item, &cleanable))
        .collect();

    Ok(AnalysisSummaryDto {
        findings,
        reclaimable_bytes: report.reclaimable(),
        safe_bytes: report.safe_bytes(),
        known_cleanable_bytes: cleanable.total_bytes(),
        source_counts: report.source_counts.clone(),
        used_remote,
        remote_needs_consent: remote_configured_without_consent(services),
    })
}

/// The persisted known-cleanable list.
#[tauri::command]
pub fn list_cleanable(services: State<'_, AppServices>) -> Vec<FindingDto> {
    let cleanable = services.store.cleanable();
    let mut entries: Vec<FindingDto> = cleanable
        .entries()
        .iter()
        .map(|entry| FindingDto {
            id: entry.key().to_string(),
            name: entry.name.clone(),
            path: entry.path.to_string_lossy().into_owned(),
            display_path: entry.display_path.clone(),
            size: entry.size,
            is_dir: entry.fingerprint.is_dir,
            safety: safety_token(entry.safety).to_string(),
            confidence: entry.confidence,
            reason: describe_reason(&entry.reason),
            reason_kind: reason_kind(&entry.reason).to_string(),
            reason_params: reason_params(&entry.reason),
            source: entry.source.label(),
            kind: entry.kind_token.clone(),
            known_cleanable: true,
            approved_for_auto: entry.approved_for_auto,
            cleanup_command: entry.cleanup_command.clone(),
            cleanup_method: entry.cleanup_method.clone(),
            impact: describe_reason(&entry.impact),
            impact_kind: reason_kind(&entry.impact).to_string(),
            impact_params: reason_params(&entry.impact),
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.size));
    entries
}

/// Approve or revoke unattended cleanup for one remembered entry.
#[tauri::command]
pub fn set_cleanable_approval(
    services: State<'_, AppServices>,
    id: String,
    approved: bool,
) -> Result<bool, String> {
    let key = sift_core::NodeKey::from_hex(&id).ok_or_else(|| "err.invalidNodeId".to_string())?;
    let changed = services.store.set_auto_approval(key, approved);
    let _ = services.store.flush_if_dirty();
    Ok(changed)
}

/// Approve every structural (non-model) conclusion at once.
#[tauri::command]
pub fn approve_structural(services: State<'_, AppServices>) -> usize {
    let approved = services.store.approve_all_structural();
    let _ = services.store.flush_if_dirty();
    approved
}

/// Forget one remembered entry.
#[tauri::command]
pub fn forget_cleanable(services: State<'_, AppServices>, id: String) -> Result<bool, String> {
    let key = sift_core::NodeKey::from_hex(&id).ok_or_else(|| "err.invalidNodeId".to_string())?;
    let removed = services.store.forget_cleanable(key);
    let _ = services.store.flush_if_dirty();
    Ok(removed)
}

/// Send paths to the trash and record the decisions.
///
/// Async and off-loaded to a blocking-task thread on purpose: the macOS remover
/// talks to Finder over a synchronous Apple Event that can take seconds (a
/// network volume, a Finder copy dialog), and running that on the async runtime
/// or the main thread freezes the UI. Per-item results are unchanged.
#[tauri::command]
pub async fn clean_paths(
    services: State<'_, AppServices>,
    paths: Vec<String>,
) -> Result<Vec<crate::cleanup::DeleteResultItem>, String> {
    let store = services.store();
    let last_report = services.last_report.lock().unwrap().clone();
    Ok(cleanup_off_thread(store, last_report, paths, false).await)
}

/// Run the blocking cleanup on a blocking-task thread, never on the async
/// runtime thread or the platform main thread.
async fn cleanup_off_thread(
    store: Arc<Store>,
    last_report: HashMap<PathBuf, ReportItem>,
    paths: Vec<String>,
    automatic: bool,
) -> Vec<crate::cleanup::DeleteResultItem> {
    // Kept for the panic fallback: the closure takes ownership of `paths`.
    let requested = paths.clone();
    tauri::async_runtime::spawn_blocking(move || {
        perform_cleanup_with(&store, &last_report, &paths, automatic)
    })
    .await
    .unwrap_or_else(|_| {
        // The task panicked without reporting. Never return nothing: answer
        // every requested path as an individual failure.
        requested
            .iter()
            .map(|path| crate::cleanup::DeleteResultItem {
                path: path.clone(),
                ok: false,
                error: Some("err.cleanupInterrupted".to_string()),
            })
            .collect()
    })
}

/// One guardrail-checked cleanup, shared by the interactive command, the
/// routine runner and the daily schedule. A path is removable when it is a
/// remembered cleanable entry or part of the last analysis and is not a `Keep`;
/// an unknown path is refused and reported, never silently dropped.
pub(crate) fn perform_cleanup(
    services: &AppServices,
    paths: &[String],
    automatic: bool,
) -> Vec<crate::cleanup::DeleteResultItem> {
    let store = services.store();
    let last_report = services.last_report.lock().unwrap();
    perform_cleanup_with(&store, &last_report, paths, automatic)
}

/// The blocking cleanup over owned/shared data. Split out so the same work can
/// run on a spawned thread (an async command, the tray menu) without borrowing
/// [`AppServices`]: the `Arc<Store>` gives shared ownership of persistence, and
/// the last-report map is snapshotted by the caller before spawning.
fn perform_cleanup_with(
    store: &Arc<Store>,
    last_report: &HashMap<PathBuf, ReportItem>,
    paths: &[String],
    automatic: bool,
) -> Vec<crate::cleanup::DeleteResultItem> {
    let cleanable = store.cleanable();
    let now = now_ms();

    struct Allowed {
        path: PathBuf,
        remembered: bool,
        item: ReportItem,
    }

    let mut allowed: Vec<Allowed> = Vec::new();
    let mut results: Vec<crate::cleanup::DeleteResultItem> = Vec::new();

    for text in paths {
        let path = PathBuf::from(text);
        let on_cleanable = cleanable.entries().iter().find(|entry| entry.path == path);

        let item = if let Some(entry) = on_cleanable {
            ReportItem {
                key: entry.key(),
                name: entry.name.clone(),
                display_path: entry.display_path.clone(),
                is_dir: entry.fingerprint.is_dir,
                size: entry.size,
                mtime_ms: entry.fingerprint.mtime_ms,
                kind_token: entry.kind_token.clone(),
                safety: entry.safety,
            }
        } else if let Some(item) = last_report.get(&path) {
            item.clone()
        } else {
            results.push(crate::cleanup::DeleteResultItem {
                path: text.clone(),
                ok: false,
                error: Some("err.deleteNotConfirmed".into()),
            });
            continue;
        };

        if item.safety == Safety::Keep {
            results.push(crate::cleanup::DeleteResultItem {
                path: text.clone(),
                ok: false,
                error: Some("err.keepRefused".into()),
            });
            continue;
        }
        // Re-verify the fingerprint immediately before deletion: the approval
        // described the object as analysed. A replacement or rewrite since then
        // is skipped, not deleted.
        if !sift_monitor::policy::fingerprint_still_matches(
            &path,
            item.size,
            item.mtime_ms,
            item.is_dir,
        ) {
            results.push(crate::cleanup::DeleteResultItem {
                path: text.clone(),
                ok: false,
                error: Some("err.fingerprintChanged".into()),
            });
            continue;
        }
        allowed.push(Allowed {
            path,
            remembered: on_cleanable.is_some(),
            item,
        });
    }

    let as_paths: Vec<PathBuf> = allowed.iter().map(|entry| entry.path.clone()).collect();
    let trash_results = sift_platform::trash::trash_paths(&as_paths);

    let mut session_bytes: u64 = 0;
    let mut session_titles: Vec<String> = Vec::new();
    for (entry, result) in allowed.into_iter().zip(trash_results) {
        if result.ok && entry.remembered {
            if let Some(cleanable_entry) = cleanable
                .entries()
                .iter()
                .find(|cleanable_entry| cleanable_entry.path == entry.path)
            {
                store.note_removed(cleanable_entry, now);
            }
        } else if result.ok {
            // A confirmed review item that never entered the cleanable list:
            // record the removal directly so habits still see it.
            let decision = Decision::new(
                sift_analyze::PathFingerprint::new(
                    entry.item.key,
                    entry.item.size,
                    entry.item.mtime_ms,
                    entry.item.is_dir,
                ),
                entry.item.display_path,
                entry.item.name.clone(),
                entry.item.kind_token,
                entry.item.size,
                DecisionOutcome::Removed,
                now,
            );
            store.record_decision(decision);
        }
        if result.ok {
            session_bytes = session_bytes.saturating_add(entry.item.size);
            session_titles.push(entry.item.name.clone());
        }
        results.push(crate::cleanup::DeleteResultItem {
            path: result.path.to_string_lossy().into_owned(),
            ok: result.ok,
            error: result.error,
        });
    }
    if !session_titles.is_empty() {
        store.note_cleanup_session(&session_titles, session_bytes, automatic, now);
    }

    let _ = store.flush_if_dirty();
    results
}

/// Current monitor state, for the status area.
#[tauri::command]
pub fn monitor_status(
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> MonitorStatusDto {
    let settings = services.store.settings();
    let confidence = sift_analyze::ConfidencePolicy::default();
    let cleanable = services.store.cleanable();
    let auto_eligible = cleanable.auto_eligible(&confidence);
    let root = manager.last_root().or_else(|| {
        manager
            .last_tree()
            .map(|tree| tree.lock().unwrap().root_path().to_path_buf())
    });
    let (available, total) = root
        .as_deref()
        .and_then(sift_platform::volume::free_space)
        .unwrap_or((0, 0));

    MonitorStatusDto {
        running: services.monitor_running(),
        enabled: settings.monitor.enabled,
        auto_mode: auto_mode_token(settings.monitor.auto_mode).to_string(),
        cleanable_items: cleanable.len(),
        auto_eligible_items: auto_eligible.len(),
        auto_eligible_bytes: cleanable.auto_eligible_bytes(&confidence),
        last_level: "unknown".to_string(),
        last_available_bytes: available,
        last_total_bytes: total,
        scheduled_cleanup_enabled: settings.monitor.scheduled_cleanup_enabled,
        scheduled_hour: settings.monitor.scheduled_hour,
    }
}

/// Configure the daily unattended cleanup: whether it runs and at which hour.
#[tauri::command]
pub fn set_scheduled_cleanup(
    services: State<'_, AppServices>,
    enabled: bool,
    hour: u32,
) -> Result<(), String> {
    if hour > 23 {
        return Err("err.invalidHour".to_string());
    }
    services.store.update_settings(|settings| {
        settings.monitor.scheduled_cleanup_enabled = enabled;
        settings.monitor.scheduled_hour = hour;
    });
    Ok(())
}

/// Start watching free space for the volume of the last scan.
#[tauri::command]
pub fn start_monitor(
    app: AppHandle,
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> Result<(), String> {
    start_monitor_with(&app, &services, &manager)
}

/// Start the monitor from code outside a command handler (the tray menu).
pub fn start_monitor_with(
    app: &AppHandle,
    services: &AppServices,
    manager: &ScanManager,
) -> Result<(), String> {
    if services.monitor_running() {
        return Ok(());
    }
    let root = manager
        .last_root()
        .ok_or_else(|| "err.noDiskSelected".to_string())?;
    let volume = sift_platform::volume::list_volumes()
        .into_iter()
        .find(|volume| root.starts_with(&volume.mount_point))
        .ok_or_else(|| "err.diskNotFound".to_string())?;

    let source: Arc<dyn CleanupSource> = services.store.clone();
    let handle = Monitor::start(volume, source, dirs_home()).map_err(|err| err.to_string())?;
    let (events, control) = handle.into_parts();

    // Forward monitor events to the front end. The control keeps the watch alive.
    let app_for_pump = app.clone();
    let receiver_handle = events;
    std::thread::spawn(move || {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            match receiver_handle.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(event) => {
                    let _ = app_for_pump.emit("monitor://event", monitor_event_json(&event));
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    *services.monitor.lock().unwrap() = Some(control);
    Ok(())
}

/// Stop the monitor.
#[tauri::command]
pub fn stop_monitor(services: State<'_, AppServices>) {
    if let Some(control) = services.monitor.lock().unwrap().take() {
        control.cancel();
    }
}

/// Switch the unattended cleanup mode: `off` | `notify` | `auto`.
#[tauri::command]
pub fn set_auto_clean_mode(services: State<'_, AppServices>, mode: String) -> Result<(), String> {
    let parsed = match mode.as_str() {
        "off" => AutoCleanMode::Off,
        "notify" => AutoCleanMode::NotifyOnly,
        "auto" => AutoCleanMode::AutoApproved,
        other => return Err(format!("err.unknownAutoMode|{other}")),
    };
    services
        .store
        .update_settings(|settings| settings.monitor.auto_mode = parsed);
    let _ = services.store.flush_if_dirty();
    Ok(())
}

/// Mark one path as a user-chosen cleanup candidate.
///
/// The path still runs through the same deletion policy as every other
/// candidate: a protected location, a bundle or an unreadable parent is
/// refused here. User-marked items are `Review` — a manual choice never
/// becomes a standing `Safe` approval.
#[tauri::command]
pub fn mark_path(
    services: State<'_, AppServices>,
    path: String,
    bytes: Option<u64>,
) -> Result<FindingDto, String> {
    use sift_core::deletable::classify_with_permissions;

    let as_path = PathBuf::from(&path);
    let verdict = classify_with_permissions(&as_path);
    if !verdict.is_yes() {
        let reason = verdict.refusal_key().unwrap_or("delete.refused.unknown");
        return Err(reason.to_string());
    }

    let meta = std::fs::symlink_metadata(&as_path).map_err(|err| err.to_string())?;
    let is_dir = meta.is_dir();

    #[cfg(unix)]
    let measured = {
        use std::os::unix::fs::MetadataExt;
        meta.len().max(meta.blocks() * 512)
    };
    #[cfg(not(unix))]
    let measured = meta.len();
    // For a directory, `symlink_metadata` only yields the directory inode's
    // own size, not its subtree. The caller supplies the measured subtree
    // size (display/accounting only; deletion is gated by mtime + type).
    let size = if is_dir {
        measured.max(bytes.unwrap_or(0))
    } else {
        measured
    };

    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);

    let key = sift_core::NodeKey::from_path(&as_path);
    let name = as_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or(path.clone());
    let home = dirs_home();
    let display = redact_path(&as_path, home.as_deref(), None);

    let item = ReportItem {
        key,
        name: name.clone(),
        display_path: display.clone(),
        is_dir,
        size,
        mtime_ms,
        kind_token: "userMarked".into(),
        safety: Safety::Review,
    };

    {
        let mut last_report = services.last_report.lock().unwrap();
        last_report.insert(as_path.clone(), item.clone());
    }

    Ok(FindingDto {
        id: key.to_string(),
        name,
        path,
        display_path: display,
        size,
        is_dir,
        safety: "review".into(),
        confidence: 1.0,
        reason: "reason.userMarked".into(),
        reason_kind: "key".into(),
        reason_params: Vec::new(),
        source: "user".into(),
        kind: "userMarked".into(),
        known_cleanable: false,
        approved_for_auto: false,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: "cleanup.impact.markedInTrash".into(),
        impact_kind: "key".into(),
        impact_params: Vec::new(),
    })
}

/// Warnings collected while loading local state, drained once.
#[tauri::command]
pub fn take_store_warnings(services: State<'_, AppServices>) -> Vec<StoreWarning> {
    std::mem::take(&mut *services.warnings.lock().unwrap())
}

/// The directory local state lives in, for a "reveal" affordance.
#[tauri::command]
pub fn store_location(services: State<'_, AppServices>) -> String {
    services.store.root_display()
}

/// Read the persisted interface language tag.
#[tauri::command]
pub fn get_language(services: State<'_, AppServices>) -> String {
    services.store.settings().language
}

/// Persist the interface language tag (`zh` | `en`).
#[tauri::command]
pub fn set_language(services: State<'_, AppServices>, language: String) -> Result<(), String> {
    let tag = match language.as_str() {
        "zh" | "en" => language,
        other => return Err(format!("err.unknownLanguage|{other}")),
    };
    services
        .store
        .update_settings(|settings| settings.language = tag);
    let _ = services.store.flush_if_dirty();
    Ok(())
}

/// Suggest routines from the persisted decisions.
#[tauri::command]
pub fn routine_suggestions(services: State<'_, AppServices>) -> Vec<RoutineSuggestionDto> {
    services
        .store
        .decisions()
        .suggest_routines(3, 3)
        .into_iter()
        .filter(|suggestion| {
            let key = sift_store::suggestion_key(&suggestion.name, &suggestion.kind_token);
            !services.store.routines().is_ignored(&key)
        })
        .map(|suggestion| {
            // Read the derived flags before moving any field out.
            let destructive = suggestion.is_destructive();
            let reason = suggestion.reason.label_key().to_string();
            let cadence = suggestion.cadence.label_key().to_string();
            RoutineSuggestionDto {
                name: suggestion.name,
                kind: suggestion.kind_token,
                occurrences: suggestion.occurrences,
                distinct_days: suggestion.distinct_days,
                average_bytes: suggestion.average_bytes,
                total_bytes: suggestion.total_bytes,
                destructive,
                reason,
                cadence,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineSuggestionDto {
    pub name: String,
    pub kind: String,
    pub occurrences: usize,
    pub distinct_days: usize,
    pub average_bytes: u64,
    pub total_bytes: u64,
    /// Whether accepting it would delete anything.
    pub destructive: bool,
    pub reason: String,
    pub cadence: String,
}

/// Read the AI provider configuration for the settings sheet.
#[tauri::command]
pub fn get_ai_config(services: State<'_, AppServices>) -> AiConfigDto {
    let ai = services.store.settings().ai;
    let has_token = ai.api_key_from_env().is_some() || crate::credentials::has_token();
    AiConfigDto {
        enabled: ai.enabled,
        provider: ai.provider,
        endpoint: ai.endpoint,
        model: ai.model,
        language: ai.language,
        batch_size: ai.batch_size,
        has_token,
    }
}

/// Persist the AI provider configuration and, when supplied, its token.
#[tauri::command]
pub fn save_ai_config(
    services: State<'_, AppServices>,
    config: SaveAiConfigInput,
) -> Result<AiConfigDto, String> {
    let endpoint = config.endpoint.trim().to_string();
    let model = config.model.trim().to_string();
    let provider = config.provider.trim().to_string();

    // Handle the token first so an enable without a secret cannot succeed
    // before storage fails below it.
    let token_after = match config.token.as_deref() {
        None => crate::credentials::has_token(),
        Some(value) if value.trim().is_empty() => {
            crate::credentials::delete_token()?;
            false
        }
        Some(value) => {
            crate::credentials::set_token(value.trim())?;
            true
        }
    };
    let env_key_present = services.store.settings().ai.api_key_from_env().is_some();

    if config.enabled && (endpoint.is_empty() || model.is_empty()) {
        return Err("err.aiEndpointRequired".to_string());
    }
    if config.enabled && !(token_after || env_key_present) {
        return Err("err.aiTokenRequired".to_string());
    }

    services.store.update_settings(|settings| {
        let ai = &mut settings.ai;
        ai.enabled = config.enabled;
        // Enabling is itself the explicit consent: the sheet shows exactly
        // what metadata leaves the machine. Disabling leaves the recorded
        // consent in place so re-enabling needs no second acknowledgement.
        if config.enabled {
            ai.consent_granted = true;
        }
        ai.provider = provider;
        ai.endpoint = endpoint;
        ai.model = model;
        if let Some(language) = config.language.as_deref() {
            if !language.trim().is_empty() {
                ai.language = language.trim().to_string();
            }
        }
    });
    let _ = services.store.flush_if_dirty();

    let ai = services.store.settings().ai;
    Ok(AiConfigDto {
        enabled: ai.enabled,
        provider: ai.provider,
        endpoint: ai.endpoint,
        model: ai.model,
        language: ai.language,
        batch_size: ai.batch_size,
        has_token: token_after || env_key_present,
    })
}

// ---- helpers ---------------------------------------------------------------

/// Build the adjudicator: local rules, plus a remote provider when the user has
/// configured one and consented to it.
fn build_adjudicator(services: &AppServices) -> Box<dyn Adjudicator> {
    let ai = services.store.settings().ai;

    #[cfg(feature = "remote-ai")]
    {
        if ai.is_usable() {
            // Prefer the environment variable; otherwise the token kept in
            // the user's Keychain by the settings sheet.
            let stored_key = ai.api_key_from_env().or_else(crate::credentials::token);
            if let Some(api_key) = stored_key {
                let config = sift_analyze::LlmConfig::new(
                    ai.provider.clone(),
                    ai.endpoint.clone(),
                    ai.model.clone(),
                    api_key,
                )
                .with_consent()
                .with_language(ai.language.clone())
                .with_batch_size(ai.batch_size);
                if let Ok(remote) = sift_analyze::OpenAiCompatibleAdjudicator::new(config) {
                    return Box::new(sift_analyze::HabitAdjudicator::new(
                        sift_analyze::RoutingAdjudicator::with_remote(
                            RuleAdjudicator::new(),
                            remote,
                            sift_analyze::Guardrails::default(),
                        ),
                        services.store.decisions(),
                        sift_analyze::HabitPolicy::default(),
                    ));
                }
            }
        }
    }
    let _ = &ai;
    // No usable real provider (no token configured): run the same pipeline
    // against the offline simulated adjudicator, so model-adjudicable families
    // still receive an AI-style judgment and impact without network access.
    Box::new(sift_analyze::HabitAdjudicator::new(
        sift_analyze::RoutingAdjudicator::with_remote(
            RuleAdjudicator::new(),
            sift_analyze::SimulatedAdjudicator::new(),
            sift_analyze::Guardrails::default(),
        ),
        services.store.decisions(),
        sift_analyze::HabitPolicy::default(),
    ))
}

fn remote_configured_without_consent(services: &AppServices) -> bool {
    let ai = services.store.settings().ai;
    (ai.enabled || !ai.endpoint.trim().is_empty()) && !ai.is_usable()
}

fn finding_dto(
    item: &sift_analyze::AnalyzedItem,
    cleanable: &sift_store::CleanableList,
) -> FindingDto {
    let key = item.candidate.key;
    let remembered = cleanable.get(key);
    FindingDto {
        id: key.to_string(),
        name: item.candidate.name.clone(),
        path: item.candidate.path.to_string_lossy().into_owned(),
        display_path: item.candidate.display_path.clone(),
        size: item.candidate.reclaimable(),
        is_dir: item.candidate.is_dir,
        safety: safety_token(item.verdict.safety).to_string(),
        confidence: item.verdict.confidence,
        reason: describe_reason(&item.verdict.reason),
        reason_kind: reason_kind(&item.verdict.reason).to_string(),
        reason_params: reason_params(&item.verdict.reason),
        source: item.verdict.source.label(),
        kind: item.candidate.kind.token().to_string(),
        known_cleanable: item.verdict.safety == Safety::Safe,
        approved_for_auto: remembered
            .map(|entry| entry.approved_for_auto)
            .unwrap_or(false),
        cleanup_command: {
            let plan = plan_for(&item.candidate.kind, &item.candidate.display_path);
            plan.command.as_ref().map(|step| display_command(step))
        },
        cleanup_method: {
            plan_for(&item.candidate.kind, &item.candidate.display_path)
                .method
                .token()
                .to_string()
        },
        impact: {
            let impact = item.verdict.impact.clone().unwrap_or_else(|| {
                Reason::key(plan_for(&item.candidate.kind, &item.candidate.display_path).impact_key)
            });
            describe_reason(&impact)
        },
        impact_kind: {
            let impact = item.verdict.impact.clone().unwrap_or_else(|| {
                Reason::key(plan_for(&item.candidate.kind, &item.candidate.display_path).impact_key)
            });
            reason_kind(&impact).to_string()
        },
        impact_params: {
            let impact = item.verdict.impact.clone().unwrap_or_else(|| {
                Reason::key(plan_for(&item.candidate.kind, &item.candidate.display_path).impact_key)
            });
            reason_params(&impact)
        },
    }
}

// ---- history & saved routines ---------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntryDto {
    pub id: String,
    pub at_ms: i64,
    pub bytes: u64,
    pub items: usize,
    pub automatic: bool,
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineDto {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub cadence: String,
    pub average_bytes: u64,
    /// `auto` | `approve`.
    pub mode: String,
    /// Explicit targets of a path-based routine; empty for a kind-based one.
    pub paths: Vec<String>,
}

/// Read the user-facing cleanup timeline, newest first.
#[tauri::command]
pub fn list_history(services: State<'_, AppServices>) -> Vec<HistoryEntryDto> {
    services
        .store
        .history()
        .entries()
        .iter()
        .map(|entry| HistoryEntryDto {
            id: entry.id.clone(),
            at_ms: entry.at_ms,
            bytes: entry.bytes,
            items: entry.items,
            automatic: entry.automatic,
            titles: entry.titles.clone(),
        })
        .collect()
}

/// Read every saved routine.
#[tauri::command]
pub fn list_routines(services: State<'_, AppServices>) -> Vec<RoutineDto> {
    services
        .store
        .routines()
        .entries()
        .iter()
        .map(|entry| RoutineDto {
            id: entry.id.clone(),
            title: entry.title.clone(),
            kind: entry.kind.clone(),
            cadence: entry.cadence.clone(),
            average_bytes: entry.average_bytes,
            mode: routine_mode_token(entry.mode).to_string(),
            paths: entry.paths.clone(),
        })
        .collect()
}

/// Persist one mined suggestion as a standing routine (approve mode default).
#[tauri::command]
pub fn accept_routine_suggestion(
    services: State<'_, AppServices>,
    name: String,
    kind: String,
) -> Result<(), String> {
    let suggestion = services
        .store
        .decisions()
        .suggest_routines(3, 3)
        .into_iter()
        .find(|suggestion| suggestion.name == name && suggestion.kind_token == kind)
        .ok_or_else(|| "err.routineSuggestionGone".to_string())?;

    let id = sift_store::suggestion_key(&suggestion.name, &suggestion.kind_token);
    services.store.upsert_routine(sift_store::RoutineEntry {
        id,
        title: suggestion.name,
        kind: suggestion.kind_token,
        cadence: suggestion.cadence.label_key().to_string(),
        average_bytes: suggestion.average_bytes,
        mode: sift_store::RoutineMode::Approve,
        paths: Vec::new(),
    });
    let _ = services.store.flush_if_dirty();
    Ok(())
}

/// Save a routine directly from the Smart list (a finding's cleanable family)
/// or from the Explorer (one explicit path). New routines start in approve
/// mode, so nothing is ever removed without a later confirmation.
#[tauri::command]
pub fn save_routine(
    services: State<'_, AppServices>,
    title: String,
    kind: String,
    average_bytes: u64,
    path: Option<String>,
) -> Result<(), String> {
    if let Some(path) = path {
        // One path-based routine per target, so adding it again updates rather
        // than duplicates.
        let key = sift_core::NodeKey::from_path(std::path::Path::new(&path));
        let id = format!("custom:{key}");
        services.store.upsert_routine(sift_store::RoutineEntry {
            id,
            title,
            kind: CUSTOM_PATH_KIND.to_string(),
            cadence: "cadence.weekly".to_string(),
            average_bytes,
            mode: sift_store::RoutineMode::Approve,
            paths: vec![path],
        });
    } else {
        if kind.is_empty() {
            return Err("err.routineKindMissing".to_string());
        }
        let id = sift_store::suggestion_key(&title, &kind);
        services.store.upsert_routine(sift_store::RoutineEntry {
            id,
            title,
            kind: kind.clone(),
            cadence: "cadence.weekly".to_string(),
            average_bytes,
            mode: sift_store::RoutineMode::Approve,
            paths: Vec::new(),
        });
        // The user chose this family as a routine; let a later manual run act on
        // its currently known Safe conclusions without another approval prompt.
        let safe_keys: Vec<sift_core::NodeKey> = services
            .store
            .cleanable()
            .entries()
            .iter()
            .filter(|entry| entry.kind_token == kind && entry.safety == Safety::Safe)
            .map(|entry| entry.key())
            .collect();
        for key in safe_keys {
            services.store.set_auto_approval(key, true);
        }
    }
    let _ = services.store.flush_if_dirty();
    Ok(())
}

/// Family token for a routine that targets explicit user-chosen paths.
pub(crate) const CUSTOM_PATH_KIND: &str = "customPath";

/// Dismiss one mined suggestion; it is not suggested again.
#[tauri::command]
pub fn dismiss_routine_suggestion(
    services: State<'_, AppServices>,
    name: String,
    kind: String,
) -> Result<(), String> {
    let key = sift_store::suggestion_key(&name, &kind);
    services.store.ignore_routine_suggestion(key);
    let _ = services.store.flush_if_dirty();
    Ok(())
}

/// Delete one saved routine.
#[tauri::command]
pub fn delete_routine(services: State<'_, AppServices>, id: String) -> Result<bool, String> {
    let removed = services.store.delete_routine(&id);
    let _ = services.store.flush_if_dirty();
    Ok(removed)
}
/// Flip one saved routine between auto-run and approve-before-run.
#[tauri::command]
pub fn toggle_routine_mode(services: State<'_, AppServices>, id: String) -> Result<bool, String> {
    let changed = services.store.toggle_routine_mode(&id);
    let _ = services.store.flush_if_dirty();
    Ok(changed)
}

/// Run one saved routine now: trash every remembered, individually approved
/// `safe` item of its kind.
#[tauri::command]
pub async fn run_routine(
    services: State<'_, AppServices>,
    id: String,
) -> Result<Vec<crate::cleanup::DeleteResultItem>, String> {
    let routine = services
        .store
        .routines()
        .get(&id)
        .cloned()
        .ok_or_else(|| "err.routineNotFound".to_string())?;

    let paths = if !routine.paths.is_empty() {
        // A path-based routine: act only on targets that still exist, leaving
        // the stored paths intact so missing ones are simply skipped.
        routine
            .paths
            .iter()
            .filter(|path| std::path::Path::new(path).exists())
            .cloned()
            .collect()
    } else {
        select_routine_paths(services.store.cleanable().entries(), &routine.kind)
    };

    if paths.is_empty() {
        return Ok(Vec::new());
    }
    // Finder IPC runs on a blocking-task thread, like clean_paths.
    let store = services.store();
    let last_report = services.last_report.lock().unwrap().clone();
    Ok(cleanup_off_thread(store, last_report, paths, true).await)
}

/// Select remembered entries a "run routine now" may act on: matching the
/// routine's family, `Safe`, and individually approved for automatic cleanup.
pub(crate) fn select_routine_paths(
    entries: &[sift_store::CleanableEntry],
    kind: &str,
) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| {
            entry.kind_token == kind
                && entry.safety == Safety::Safe
                && entry.approved_for_auto
        })
        .map(|entry| entry.path.to_string_lossy().into_owned())
        .collect()
}

fn safety_token(safety: Safety) -> &'static str {
    match safety {
        Safety::Safe => "safe",
        Safety::Review => "review",
        Safety::Keep => "keep",
    }
}

fn auto_mode_token(mode: AutoCleanMode) -> &'static str {
    match mode {
        AutoCleanMode::Off => "off",
        AutoCleanMode::NotifyOnly => "notify",
        AutoCleanMode::AutoApproved => "auto",
    }
}

fn routine_mode_token(mode: sift_store::RoutineMode) -> &'static str {
    match mode {
        sift_store::RoutineMode::Auto => "auto",
        sift_store::RoutineMode::Approve => "approve",
    }
}

fn reason_kind(reason: &Reason) -> &'static str {
    if reason.is_text() {
        "text"
    } else {
        "key"
    }
}

fn describe_reason(reason: &Reason) -> String {
    match reason {
        Reason::Key { key, .. } => key.clone(),
        Reason::Text(text) => text.clone(),
    }
}

fn reason_params(reason: &Reason) -> Vec<String> {
    match reason {
        Reason::Key { params, .. } => params.clone(),
        Reason::Text(_) => Vec::new(),
    }
}

fn monitor_event_json(event: &MonitorEvent) -> serde_json::Value {
    match event {
        MonitorEvent::Sampled {
            level,
            sample,
            reclaimable_bytes,
            entry_count,
        } => serde_json::json!({
            "kind": "sampled",
            "level": level_token(*level),
            "availableBytes": sample.available_bytes,
            "totalBytes": sample.total_bytes,
            "reclaimableBytes": reclaimable_bytes,
            "entryCount": entry_count,
        }),
        MonitorEvent::Notify {
            level,
            available_bytes,
            reclaimable_bytes,
            entry_count,
        } => serde_json::json!({
            "kind": "notify",
            "level": level_token(*level),
            "availableBytes": available_bytes,
            "reclaimableBytes": reclaimable_bytes,
            "entryCount": entry_count,
        }),
        MonitorEvent::ConfirmationNeeded {
            level,
            reason,
            reclaimable_bytes,
            entry_count,
        } => serde_json::json!({
            "kind": "confirmationNeeded",
            "level": level_token(*level),
            "reason": reason.label_key(),
            "reclaimableBytes": reclaimable_bytes,
            "entryCount": entry_count,
        }),
        MonitorEvent::CleanupStarted { entry_count, bytes } => serde_json::json!({
            "kind": "cleanupStarted",
            "entryCount": entry_count,
            "bytes": bytes,
        }),
        MonitorEvent::CleanupFinished {
            removed,
            skipped,
            failed,
            bytes,
            paths,
        } => serde_json::json!({
            "kind": "cleanupFinished",
            "removed": removed,
            "skipped": skipped,
            "failed": failed,
            "bytes": bytes,
            "paths": paths,
        }),
        MonitorEvent::Warning(message) => serde_json::json!({
            "kind": "warning",
            "message": message,
        }),
        // `MonitorEvent` is non-exhaustive: a future variant must not break the
        // front end, and must not be silently dropped either.
        _ => serde_json::json!({ "kind": "unknown" }),
    }
}

fn level_token(level: sift_monitor::AlertLevel) -> &'static str {
    match level {
        sift_monitor::AlertLevel::Normal => "normal",
        sift_monitor::AlertLevel::Warning => "warning",
        sift_monitor::AlertLevel::Critical => "critical",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn dirs_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Compatibility re-export so `lib.rs` can clear state on shutdown.
pub fn shutdown(app: &AppHandle) {
    if let Some(services) = app.try_state::<AppServices>() {
        if let Some(control) = services.monitor.lock().unwrap().take() {
            control.cancel();
        }
        let _ = services.store.flush_if_dirty();
    }
    let _ = AnalysisStage::Done;
    let _ = VerdictSource::Cached;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_load_without_warnings_on_a_clean_machine() {
        let services = AppServices::new();
        assert!(!services.monitor_running());
        assert!(!services.store.root_display().is_empty());
    }

    #[test]
    fn safety_and_auto_mode_tokens_are_stable() {
        assert_eq!(safety_token(Safety::Safe), "safe");
        assert_eq!(safety_token(Safety::Review), "review");
        assert_eq!(safety_token(Safety::Keep), "keep");
        assert_eq!(auto_mode_token(AutoCleanMode::Off), "off");
        assert_eq!(auto_mode_token(AutoCleanMode::NotifyOnly), "notify");
        assert_eq!(auto_mode_token(AutoCleanMode::AutoApproved), "auto");
    }

    #[test]
    fn reasons_are_transported_with_their_kind() {
        let keyed = Reason::key_with("reason.rebuildableCache", vec!["npm".into()]);
        assert_eq!(reason_kind(&keyed), "key");
        assert_eq!(describe_reason(&keyed), "reason.rebuildableCache");
        assert_eq!(reason_params(&keyed), vec!["npm".to_string()]);

        let text = Reason::text("an old installer");
        assert_eq!(reason_kind(&text), "text");
        assert_eq!(describe_reason(&text), "an old installer");
        assert!(reason_params(&text).is_empty());
    }

    #[test]
    fn monitor_events_serialise_with_a_kind_tag() {
        let event = MonitorEvent::CleanupFinished {
            removed: 2,
            skipped: 1,
            failed: 0,
            bytes: 4096,
            paths: vec![PathBuf::from("/tmp/a")],
        };
        let json = monitor_event_json(&event);
        assert_eq!(json["kind"], "cleanupFinished");
        assert_eq!(json["removed"], 2);
        assert_eq!(json["skipped"], 1);
        assert_eq!(json["bytes"], 4096);
    }

    #[test]
    fn a_local_adjudicator_is_built_without_configuration() {
        let services = AppServices::new();
        // Default settings have AI disabled, so the local rules are used and
        // nothing is transmitted.
        assert!(!services.store.settings().ai.is_usable());
        assert!(!remote_configured_without_consent(&services));
    }

    #[test]
    fn perform_cleanup_skips_items_whose_fingerprint_changed() {
        let services = AppServices::new();
        let dir = std::env::temp_dir().join(format!("sift-cleanup-fp-{:?}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("stale.bin");
        std::fs::write(&path, b"hello").unwrap();
        let original = std::fs::symlink_metadata(&path).unwrap();

        // Register the item in the last report with its ORIGINAL fingerprint.
        let item = ReportItem {
            key: sift_core::NodeKey::from_bytes(b"stale"),
            name: "stale.bin".into(),
            display_path: "~/tmp/stale.bin".into(),
            is_dir: false,
            size: original.len(),
            mtime_ms: meta_mtime_ms(&original),
            kind_token: "installer".into(),
            safety: Safety::Review,
        };
        services
            .last_report
            .lock()
            .unwrap()
            .insert(path.clone(), item);

        // Rewrite after analysis: same path, different object.
        std::fs::write(&path, b"a much longer replacement body").unwrap();

        let text = path.to_string_lossy().into_owned();
        let results = perform_cleanup(&services, &[text], false);

        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert_eq!(results[0].error.as_deref(), Some("err.fingerprintChanged"));
        assert!(path.exists(), "the changed file must not be deleted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn meta_mtime_ms(meta: &std::fs::Metadata) -> i64 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.mtime() * 1000 + meta.mtime_nsec() / 1_000_000
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            0
        }
    }

    fn routine_entry(kind: &str, safety: Safety, approved: bool) -> sift_store::CleanableEntry {
        sift_store::CleanableEntry {
            fingerprint: sift_analyze::PathFingerprint::new(
                sift_core::NodeKey::from_bytes(kind.as_bytes()),
                1,
                0,
                true,
            ),
            path: PathBuf::from(format!("/Users/me/project/{kind}")),
            display_path: format!("~/project/{kind}"),
            name: kind.to_string(),
            kind_token: kind.into(),
            size: 1,
            safety,
            confidence: 0.9,
            reason: Reason::key("reason.rebuildableCache"),
            source: VerdictSource::rule("dir.node_modules"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: approved,
            cleanup_command: None,
            cleanup_method: "trashItem".into(),
            impact: Reason::key("reason.rebuildableCache"),
        }
    }

    #[test]
    fn routine_runs_only_safe_approved_items_of_its_kind() {
        let entries = vec![
            routine_entry("rebuildableCache", Safety::Safe, true),
            routine_entry("rebuildableCache", Safety::Safe, false),
            routine_entry("rebuildableCache", Safety::Review, true),
            routine_entry("otherKind", Safety::Safe, true),
        ];
        let paths = select_routine_paths(&entries, "rebuildableCache");
        assert_eq!(paths, vec!["/Users/me/project/rebuildableCache"]);
    }
}
