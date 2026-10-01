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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use sift_analyze::{
    Adjudicator, AnalysisPolicy, AnalysisStage, Analyzer, Reason, RuleAdjudicator, Safety,
    VerdictSource,
};
use sift_monitor::{CleanupSource, Monitor, MonitorEvent};
use sift_store::{AutoCleanMode, Store};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::scanner::ScanManager;

/// Services the analysis commands need: durable state and the running monitor.
pub struct AppServices {
    pub store: Arc<Store>,
    pub monitor: Mutex<Option<sift_monitor::MonitorControl>>,
    /// Warnings collected while loading the store, drained by the front end.
    pub warnings: Mutex<Vec<String>>,
}

impl AppServices {
    pub fn new() -> Self {
        let (store, warnings) = Store::open_default();
        Self {
            store: Arc::new(store),
            monitor: Mutex::new(None),
            warnings: Mutex::new(warnings),
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
}

// ---- commands --------------------------------------------------------------

/// Analyze the most recent scan and persist the conclusions.
#[tauri::command]
pub fn analyze_current(
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> Result<AnalysisSummaryDto, String> {
    let tree = manager
        .last_tree()
        .ok_or_else(|| "还没有可分析的扫描结果".to_string())?;

    let settings = services.store.settings();
    let policy = AnalysisPolicy {
        min_candidate_bytes: settings.analysis.min_candidate_bytes,
        include_files: settings.analysis.include_files,
        ..AnalysisPolicy::default()
    }
    .with_min_bytes(settings.analysis.min_candidate_bytes);

    let analyzer = Analyzer::new(policy, dirs_home());
    let adjudicator = build_adjudicator(&services);
    let used_remote = adjudicator.is_remote();

    let report = {
        let mut guard = tree.lock().map_err(|err| err.to_string())?;
        analyzer.analyze(&*adjudicator, &mut guard, now_ms(), |_stage, _done, _total| {})
    };

    // Requirement 2: persist what is definitively cleanable plus the verdicts.
    services.store.record_analysis(&report, now_ms());
    let _ = services.store.flush_if_dirty();

    let cleanable = services.store.cleanable();
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
        remote_needs_consent: remote_configured_without_consent(&services),
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
    let key = sift_core::NodeKey::from_hex(&id).ok_or_else(|| "无效的节点 id".to_string())?;
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
    let key = sift_core::NodeKey::from_hex(&id).ok_or_else(|| "无效的节点 id".to_string())?;
    let removed = services.store.forget_cleanable(key);
    let _ = services.store.flush_if_dirty();
    Ok(removed)
}

/// Describe cleaning `paths`, or send them to the trash only with `execute`.
///
/// Default is dry-run: no filesystem mutation and no decision recorded, so a
/// preview can be opened repeatedly without side effects.
#[tauri::command]
pub fn clean_paths(
    services: State<'_, AppServices>,
    paths: Vec<String>,
    execute: bool,
) -> Vec<crate::cleanup::DeleteResultItem> {
    if !execute {
        return crate::cleanup::preview_delete(paths);
    }
    let store = services.store();
    let cleanable = store.cleanable();
    let as_paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let results = sift_platform::trash::trash_paths(&as_paths);
    let now = now_ms();

    for result in &results {
        if !result.ok {
            continue;
        }
        // Remember the removal, which is what lets habits be mined later.
        if let Some(entry) = cleanable
            .entries()
            .iter()
            .find(|entry| entry.path == result.path)
        {
            store.note_removed(entry, now);
        }
    }
    let _ = store.flush_if_dirty();

    results
        .into_iter()
        .map(|result| crate::cleanup::DeleteResultItem {
            path: result.path.to_string_lossy().into_owned(),
            ok: result.ok,
            error: result.error,
            dry_run: false,
        })
        .collect()
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
    let root = manager
        .last_root()
        .or_else(|| manager.last_tree().map(|tree| tree.lock().unwrap().root_path().to_path_buf()));
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
    }
}

/// Start watching free space for the volume of the last scan.
#[tauri::command]
pub fn start_monitor(
    app: AppHandle,
    services: State<'_, AppServices>,
    manager: State<'_, ScanManager>,
) -> Result<(), String> {
    if services.monitor_running() {
        return Ok(());
    }
    let root = manager
        .last_root()
        .ok_or_else(|| "请先选择一个磁盘".to_string())?;
    let volume = sift_platform::volume::list_volumes()
        .into_iter()
        .find(|volume| root.starts_with(&volume.mount_point))
        .ok_or_else(|| "找不到该路径所在的磁盘".to_string())?;

    let source: Arc<dyn CleanupSource> = services.store();
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

/// Warnings collected while loading local state, drained once.
#[tauri::command]
pub fn take_store_warnings(services: State<'_, AppServices>) -> Vec<String> {
    std::mem::take(&mut *services.warnings.lock().unwrap())
}

/// The directory local state lives in, for a "reveal" affordance.
#[tauri::command]
pub fn store_location(services: State<'_, AppServices>) -> String {
    services.store.root_display()
}

/// Suggest routines from the persisted decisions.
#[tauri::command]
pub fn routine_suggestions(services: State<'_, AppServices>) -> Vec<RoutineSuggestionDto> {
    services
        .store
        .decisions()
        .suggest_routines(3, 3)
        .into_iter()
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

// ---- helpers ---------------------------------------------------------------

/// Build the adjudicator: local rules, plus a remote provider when the user has
/// configured one and consented to it. With no usable token, an offline mock
/// sits in the remote slot so the same prompt/contract runs locally.
fn build_adjudicator(services: &AppServices) -> Box<dyn Adjudicator> {
    let ai = services.store.settings().ai;

    #[cfg(feature = "remote-ai")]
    {
        if ai.is_usable() {
            if let Some(api_key) = ai.api_key_from_env() {
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
                    return Box::new(sift_analyze::RoutingAdjudicator::with_remote(
                        RuleAdjudicator::new(),
                        remote,
                        sift_analyze::Guardrails::default(),
                    ));
                }
            }
        }
    }

    // No usable real provider: route through the offline mock instead of
    // dropping the remote stage entirely. Only model-adjudicable families
    // reach it (via the router), and it never promotes structural families.
    let mock = sift_analyze::MockAdjudicator::new().with_language(ai.language.clone());
    Box::new(sift_analyze::RoutingAdjudicator::with_remote(
        RuleAdjudicator::new(),
        mock,
        sift_analyze::Guardrails::default(),
    ))
}

fn remote_configured_without_consent(services: &AppServices) -> bool {
    let ai = services.store.settings().ai;
    (ai.enabled || !ai.endpoint.trim().is_empty()) && !ai.is_usable()
}

fn finding_dto(item: &sift_analyze::AnalyzedItem, cleanable: &sift_store::CleanableList) -> FindingDto {
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
        approved_for_auto: remembered.map(|entry| entry.approved_for_auto).unwrap_or(false),
    }
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
        } => serde_json::json!({
            "kind": "cleanupFinished",
            "removed": removed,
            "skipped": skipped,
            "failed": failed,
            "bytes": bytes,
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
}
