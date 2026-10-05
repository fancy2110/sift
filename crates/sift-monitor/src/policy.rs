//! The monitor's decision, as a pure function.
//!
//! Everything that decides whether Sift may delete something without being
//! asked lives in [`evaluate`]. It takes a sample, the remembered cleanable
//! entries, the settings, and the cooldown state, and returns exactly one
//! [`MonitorDecision`]. No clock reads, no filesystem calls, no threads — so the
//! rules that protect a user's files can be read in one screen and tested
//! exhaustively.
//!
//! The invariants this function enforces:
//!
//! 1. Nothing is selected that is not `Safe` **and** explicitly approved.
//! 2. Nothing outside the user's home is selected while `home_only` is set.
//! 3. A batch larger than `max_auto_clean_bytes` is never split and never
//!    silently trimmed: it becomes a confirmation request.
//! 4. Nothing is selected twice inside the cooldown window.
//! 5. `NotifyOnly` (and `Off`) never select anything at all.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sift_analyze::ConfidencePolicy;
use sift_store::{AutoCleanMode, CleanableEntry, MonitorSettings};

/// How urgent the free-space situation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertLevel {
    /// Plenty of space.
    Normal,
    /// Below the warning threshold.
    Warning,
    /// Below the critical threshold.
    Critical,
}

impl AlertLevel {
    pub const fn is_normal(self) -> bool {
        matches!(self, AlertLevel::Normal)
    }

    /// i18n key for the level's label.
    pub const fn label_key(self) -> &'static str {
        match self {
            AlertLevel::Normal => "alert.normal",
            AlertLevel::Warning => "alert.warning",
            AlertLevel::Critical => "alert.critical",
        }
    }
}

/// One free-space reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DiskSample {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub taken_at_ms: i64,
}

impl DiskSample {
    pub fn new(total_bytes: u64, available_bytes: u64, taken_at_ms: i64) -> Self {
        Self {
            total_bytes,
            available_bytes: available_bytes.min(total_bytes),
            taken_at_ms,
        }
    }

    /// Free space as a fraction, clamped to `0.0..=1.0`.
    pub fn free_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        (self.available_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }

    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }
}

/// The policy inputs, gathered from settings.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MonitorConfig {
    pub enabled: bool,
    pub warn_free_ratio: f64,
    pub critical_free_ratio: f64,
    pub min_free_bytes: u64,
    pub auto_mode: AutoCleanMode,
    pub clean_cooldown_ms: i64,
    pub notify_cooldown_ms: i64,
    pub max_auto_clean_bytes: u64,
    pub home_only: bool,
    pub confidence: ConfidencePolicy,
}

impl MonitorConfig {
    /// Build from persisted settings.
    pub fn from_settings(settings: &MonitorSettings) -> Self {
        Self {
            enabled: settings.enabled,
            warn_free_ratio: settings.warn_free_ratio,
            critical_free_ratio: settings.critical_free_ratio,
            min_free_bytes: settings.min_free_bytes,
            auto_mode: settings.auto_mode,
            clean_cooldown_ms: (settings.clean_cooldown_secs as i64).saturating_mul(1000),
            notify_cooldown_ms: (settings.notify_cooldown_secs as i64).saturating_mul(1000),
            max_auto_clean_bytes: settings.max_auto_clean_bytes,
            home_only: settings.home_only,
            confidence: ConfidencePolicy::default(),
        }
    }

    /// The alert level for a sample.
    pub fn level_for(&self, sample: &DiskSample) -> AlertLevel {
        if sample.total_bytes == 0 {
            return AlertLevel::Normal;
        }
        let ratio = sample.free_ratio();
        if ratio <= self.critical_free_ratio {
            AlertLevel::Critical
        } else if ratio <= self.warn_free_ratio || sample.available_bytes <= self.min_free_bytes {
            AlertLevel::Warning
        } else {
            AlertLevel::Normal
        }
    }
}

/// Why a batch needs the user's confirmation rather than being acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfirmationReason {
    /// Reclaiming it in one go exceeds `max_auto_clean_bytes`.
    ExceedsSizeLimit,
    /// Entries reachable without confirmation exist, but the mode is not
    /// `AutoApproved`.
    ModeRequiresConsent,
}

impl ConfirmationReason {
    pub const fn label_key(self) -> &'static str {
        match self {
            ConfirmationReason::ExceedsSizeLimit => "monitor.confirm.exceedsSizeLimit",
            ConfirmationReason::ModeRequiresConsent => "monitor.confirm.modeRequiresConsent",
        }
    }
}

/// What the monitor concluded should happen.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MonitorAction {
    /// Nothing to say or do.
    Quiet,
    /// Tell the user, but do not act.
    Notify,
    /// Remove these entries unattended. Always `Safe`, always approved.
    AutoClean { entries: Vec<CleanableEntry> },
    /// Ask first, showing these entries.
    NeedsConfirmation {
        reason: ConfirmationReason,
        entries: Vec<CleanableEntry>,
    },
}

/// The full conclusion for one sample.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MonitorDecision {
    pub level: AlertLevel,
    pub action: MonitorAction,
    pub sample: DiskSample,
    /// Bytes the selected entries would reclaim.
    pub reclaimable_bytes: u64,
    /// Whether a notification was suppressed by the notify cooldown.
    pub notification_suppressed: bool,
}

impl MonitorDecision {
    pub fn is_quiet(&self) -> bool {
        matches!(self.action, MonitorAction::Quiet)
    }

    /// Whether this decision removes anything.
    pub fn deletes(&self) -> bool {
        matches!(self.action, MonitorAction::AutoClean { .. })
    }

    pub fn selected(&self) -> &[CleanableEntry] {
        match &self.action {
            MonitorAction::AutoClean { entries }
            | MonitorAction::NeedsConfirmation { entries, .. } => entries,
            _ => &[],
        }
    }
}

impl Default for MonitorDecision {
    fn default() -> Self {
        Self {
            level: AlertLevel::Normal,
            action: MonitorAction::Quiet,
            sample: DiskSample::new(0, 0, 0),
            reclaimable_bytes: 0,
            notification_suppressed: false,
        }
    }
}

/// Decide what to do about one disk sample.
///
/// `last_clean_ms` and `last_notify_ms` are the previous actions' timestamps, or
/// `None` if this is the first sample of the session.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    config: &MonitorConfig,
    sample: &DiskSample,
    entries: &[CleanableEntry],
    home: Option<&Path>,
    last_clean_ms: Option<i64>,
    last_notify_ms: Option<i64>,
    now_ms: i64,
) -> MonitorDecision {
    let level = config.level_for(sample);

    // Step 1: which entries are eligible at all, before policy.
    let eligible: Vec<CleanableEntry> = entries
        .iter()
        .filter(|entry| entry.is_auto_eligible(&config.confidence))
        .filter(|entry| !config.home_only || inside_home(&entry.path, home))
        .cloned()
        .collect();

    // Step 2: approval mode. Off and NotifyOnly never select anything.
    let can_act = config.enabled && config.auto_mode.may_delete();
    let selected = if can_act {
        eligible.clone()
    } else {
        Vec::new()
    };
    let eligible_bytes: u64 = eligible.iter().map(|entry| entry.size).sum();
    let selected_bytes: u64 = selected.iter().map(|entry| entry.size).sum();

    let base = |action: MonitorAction, reclaimable: u64, suppressed: bool| MonitorDecision {
        level,
        action,
        sample: *sample,
        reclaimable_bytes: reclaimable,
        notification_suppressed: suppressed,
    };

    // Step 3: a healthy disk is left alone, even if entries are queued.
    if level.is_normal() {
        return base(MonitorAction::Quiet, eligible_bytes, false);
    }

    if !config.enabled {
        return base(MonitorAction::Quiet, eligible_bytes, false);
    }

    let notify_ready = match last_notify_ms {
        None => true,
        Some(previous) => now_ms.saturating_sub(previous) >= config.notify_cooldown_ms,
    };

    // Step 4: act only when the mode allows it, the cooldown has elapsed, and
    // there is something to act on.
    if can_act && !selected.is_empty() {
        let cooled_down = match last_clean_ms {
            None => true,
            Some(previous) => now_ms.saturating_sub(previous) >= config.clean_cooldown_ms,
        };
        if cooled_down {
            if selected_bytes > config.max_auto_clean_bytes {
                // Too large to do quietly: never split, never trim.
                return base(
                    MonitorAction::NeedsConfirmation {
                        reason: ConfirmationReason::ExceedsSizeLimit,
                        entries: selected,
                    },
                    selected_bytes,
                    !notify_ready,
                );
            }
            return base(
                MonitorAction::AutoClean { entries: selected },
                selected_bytes,
                false,
            );
        }
    }

    // Step 5: otherwise, say something — subject to the notification cooldown.
    if !notify_ready {
        return base(MonitorAction::Quiet, eligible_bytes, true);
    }

    // If the entries exist but cannot be acted on unattended, the notification
    // is a confirmation request rather than a status line.
    if !eligible.is_empty() && !can_act {
        return base(
            MonitorAction::NeedsConfirmation {
                reason: ConfirmationReason::ModeRequiresConsent,
                entries: eligible,
            },
            eligible_bytes,
            false,
        );
    }

    base(MonitorAction::Notify, eligible_bytes, false)
}

/// Whether `path` sits inside `home`.
///
/// Both sides are lexically normalized (collapsing `.`/`..`) before the prefix
/// test, so a path like `<home>/../elsewhere` cannot smuggle an outside entry
/// past the boundary. A missing home means "cannot prove it is inside", which
/// refuses the entry: the safe direction for an unattended deletion.
///
/// Note this stays purely lexical — it cannot see through symlinks. Callers
/// that need to prove where a path actually points use
/// [`resolved_inside_home`] instead.
pub fn inside_home(path: &Path, home: Option<&Path>) -> bool {
    let Some(home) = home else {
        return false;
    };
    lexical_normalize(path).starts_with(lexical_normalize(home))
}

/// Filesystem-proven variant of [`inside_home`].
///
/// Both paths are canonicalized (`realpath`) first, so a symlink that points
/// at something outside the home cannot keep an entry inside the boundary by
/// looking home-shaped lexically. If either side cannot be resolved — the path
/// vanished, a component is not searchable — the answer is `false`: "cannot
/// prove it is inside" always refuses an unattended deletion.
pub fn resolved_inside_home(path: &Path, home: Option<&Path>) -> bool {
    let Some(home) = home else {
        return false;
    };
    let Ok(real_home) = std::fs::canonicalize(home) else {
        return false;
    };
    let Ok(real_path) = std::fs::canonicalize(path) else {
        return false;
    };
    real_path.starts_with(real_home)
}

/// Collapse `.`/`..` components without touching the filesystem.
///
/// A `..` that would escape past the path's own root cannot be resolved and is
/// left in place; such a path then only matches a home sharing the same
/// unresolvable prefix, which a real home never does.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(
                    out.components().next_back(),
                    Some(Component::Normal(_))
                );
                if can_pop {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Compare a remembered entry against the filesystem *now*.
///
/// An approval was given for one object. If the path now holds something else,
/// the approval does not describe it, and deleting it would be exactly the kind
/// of mistake this product exists to avoid.
///
/// What "the same object" means differs by kind, and the difference matters:
///
/// * **A file** is compared exactly — size and modification time. Both sides are
///   the file's own `st_size`, so a rewritten or replaced file is caught.
/// * **A directory** is compared by modification time and kind only. The size the
///   product remembers for a directory is its *aggregated subtree total*, not
///   `st_size` (which is 0 on APFS and 128 elsewhere — meaningless either way),
///   so there is no comparable size. Recomputing the total would mean walking
///   the subtree again, which is the work the monitor exists to avoid; a
///   directory's mtime changing when its direct children change is the signal
///   available at this cost.
///
/// A missing path never matches, so an entry whose target is already gone is
/// skipped rather than reported as a failure.
pub fn still_matches(entry: &CleanableEntry) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(&entry.path) else {
        return false;
    };
    if meta.is_dir() != entry.fingerprint.is_dir {
        return false;
    }
    if modified_ms(&meta) != entry.fingerprint.mtime_ms {
        return false;
    }
    if !entry.fingerprint.is_dir && meta.len() != entry.fingerprint.size {
        return false;
    }
    true
}

pub(crate) fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.mtime()
            .saturating_mul(1000)
            .saturating_add(meta.mtime_nsec() / 1_000_000)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        filetime_to_unix_ms(meta.last_write_time())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        0
    }
}

/// Convert a Windows FILETIME (100 ns ticks since 1601-01-01 UTC) to Unix
/// epoch milliseconds. Lives outside `#[cfg(windows)]` so the arithmetic can
/// be unit-tested on any platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn filetime_to_unix_ms(filetime: u64) -> i64 {
    ((filetime as i128 - 116_444_736_000_000_000) / 10_000) as i64
}

/// A path kept for tests and callers that need a plain `PathBuf` list.
pub fn paths_of(entries: &[CleanableEntry]) -> Vec<PathBuf> {
    entries.iter().map(|entry| entry.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::{PathFingerprint, Reason, Safety, VerdictSource};
    use sift_core::{ByteSize, NodeKey};

    const GB: u64 = 1024 * 1024 * 1024;
    const HOUR_MS: i64 = 3_600_000;

    fn config() -> MonitorConfig {
        MonitorConfig {
            enabled: true,
            warn_free_ratio: 0.15,
            critical_free_ratio: 0.05,
            min_free_bytes: 0,
            auto_mode: AutoCleanMode::NotifyOnly,
            clean_cooldown_ms: HOUR_MS,
            notify_cooldown_ms: 30 * 60 * 1000,
            max_auto_clean_bytes: 20 * GB,
            home_only: true,
            confidence: ConfidencePolicy::default(),
        }
    }

    fn entry(name: &str, size: u64, approved: bool, source: VerdictSource) -> CleanableEntry {
        CleanableEntry {
            fingerprint: PathFingerprint::new(NodeKey::from_bytes(name.as_bytes()), size, 0, true),
            path: PathBuf::from("/Users/me/project").join(name),
            display_path: format!("~/project/{name}"),
            name: name.to_string(),
            kind_token: "rebuildableCache".into(),
            size,
            safety: Safety::Safe,
            confidence: 0.9,
            reason: Reason::key("reason.rebuildableCache"),
            source,
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: approved,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: Reason::key("reason.rebuildableCache"),
        }
    }

    fn rule_entry(name: &str, size: u64, approved: bool) -> CleanableEntry {
        entry(
            name,
            size,
            approved,
            VerdictSource::rule("dir.node_modules"),
        )
    }

    fn sample(free_gb: u64, total_gb: u64) -> DiskSample {
        DiskSample::new(total_gb * GB, free_gb * GB, 0)
    }

    #[test]
    fn levels_follow_the_thresholds() {
        let config = config();
        assert_eq!(config.level_for(&sample(500, 1000)), AlertLevel::Normal);
        assert_eq!(config.level_for(&sample(140, 1000)), AlertLevel::Warning);
        assert_eq!(config.level_for(&sample(40, 1000)), AlertLevel::Critical);
        // A zero-capacity volume must not be reported as critical.
        assert_eq!(
            config.level_for(&DiskSample::new(0, 0, 0)),
            AlertLevel::Normal
        );
    }

    #[test]
    fn the_absolute_floor_can_trigger_a_warning_on_a_small_volume() {
        let mut config = config();
        config.min_free_bytes = 10 * GB;
        // 50% free on a small disk, but only 5 GB absolute.
        let decision = evaluate(
            &config,
            &DiskSample::new(10 * GB, 5 * GB, 0),
            &[],
            None,
            None,
            None,
            0,
        );
        assert_eq!(decision.level, AlertLevel::Warning);
    }

    #[test]
    fn a_healthy_disk_is_left_alone() {
        let decision = evaluate(
            &config(),
            &sample(500, 1000),
            &[rule_entry("node_modules", 5 * GB, true)],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(decision.is_quiet());
        assert!(!decision.deletes());
    }

    #[test]
    fn notify_only_never_deletes() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::NotifyOnly;
        let decision = evaluate(
            &config,
            // 10% free: below the 15% warning line, above the 5% critical one.
            &sample(100, 1000),
            &[rule_entry("node_modules", 5 * GB, true)],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(!decision.deletes());
        assert_eq!(decision.level, AlertLevel::Warning);
        // An approved entry exists but the mode forbids acting: ask instead.
        assert!(matches!(
            decision.action,
            MonitorAction::NeedsConfirmation {
                reason: ConfirmationReason::ModeRequiresConsent,
                ..
            }
        ));
    }

    #[test]
    fn off_mode_is_silent_even_when_critical() {
        let mut config = config();
        config.enabled = false;
        let decision = evaluate(
            &config,
            &sample(10, 1000),
            &[rule_entry("node_modules", 5 * GB, true)],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(decision.is_quiet());
    }

    #[test]
    fn auto_mode_cleans_only_approved_safe_entries() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let entries = vec![
            rule_entry("approved", GB, true),
            rule_entry("unapproved", 2 * GB, false),
        ];
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &entries,
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(decision.deletes());
        let selected = decision.selected();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "approved");
        assert_eq!(decision.reclaimable_bytes, GB);
    }

    #[test]
    fn an_oversized_batch_asks_instead_of_trimming() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        config.max_auto_clean_bytes = 4 * GB;
        let entries = vec![rule_entry("a", 3 * GB, true), rule_entry("b", 3 * GB, true)];
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &entries,
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(!decision.deletes(), "must not act above the size limit");
        match decision.action {
            MonitorAction::NeedsConfirmation { reason, entries } => {
                assert_eq!(reason, ConfirmationReason::ExceedsSizeLimit);
                // The whole batch is presented, not a silent subset.
                assert_eq!(entries.len(), 2);
            }
            other => panic!("expected a confirmation, got {other:?}"),
        }
    }

    #[test]
    fn entries_outside_home_are_never_selected() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let mut outside = rule_entry("outside", GB, true);
        outside.path = PathBuf::from("/Volumes/Backup/other");
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[outside],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(!decision.deletes());
        assert!(decision.selected().is_empty());
    }

    #[test]
    fn a_missing_home_refuses_everything() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[rule_entry("node_modules", GB, true)],
            None,
            None,
            None,
            0,
        );
        assert!(!decision.deletes(), "cannot prove it is inside home");
    }

    #[test]
    fn home_only_can_be_disabled_explicitly() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        config.home_only = false;
        let mut outside = rule_entry("outside", GB, true);
        outside.path = PathBuf::from("/Volumes/Backup/other");
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[outside],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(decision.deletes());
    }

    #[test]
    fn the_clean_cooldown_blocks_a_second_pass() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let entries = [rule_entry("node_modules", GB, true)];

        // First pass acts.
        let first = evaluate(
            &config,
            &sample(50, 1000),
            &entries,
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(first.deletes());

        // A minute later, inside the cooldown: no second deletion.
        let second = evaluate(
            &config,
            &sample(49, 1000),
            &entries,
            Some(Path::new("/Users/me")),
            Some(0),
            Some(0),
            60_000,
        );
        assert!(!second.deletes());

        // After the cooldown it may act again.
        let third = evaluate(
            &config,
            &sample(48, 1000),
            &entries,
            Some(Path::new("/Users/me")),
            Some(0),
            Some(0),
            HOUR_MS + 1,
        );
        assert!(third.deletes());
    }

    #[test]
    fn the_notify_cooldown_suppresses_repeats_without_hiding_the_level() {
        let decision = evaluate(
            &config(),
            &sample(100, 1000),
            &[],
            Some(Path::new("/Users/me")),
            None,
            Some(0),
            60_000,
        );
        assert!(decision.is_quiet());
        assert!(decision.notification_suppressed);
        // The caller can still render the level.
        assert_eq!(decision.level, AlertLevel::Warning);
    }

    #[test]
    fn low_confidence_model_conclusions_are_not_auto_eligible() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let mut weak = entry("ai", GB, true, VerdictSource::remote("test"));
        weak.confidence = 0.5; // below the remote floor
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[weak],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(
            !decision.deletes(),
            "a low-confidence model verdict is not enough"
        );
    }

    #[test]
    fn a_confident_model_conclusion_can_be_selected() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let mut confident = entry("ai", GB, true, VerdictSource::remote("test"));
        confident.confidence = 0.95;
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[confident],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(decision.deletes());
    }

    #[test]
    fn review_entries_are_never_selected() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let mut review = rule_entry("build", GB, true);
        review.safety = Safety::Review;
        let decision = evaluate(
            &config,
            &sample(50, 1000),
            &[review],
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert!(!decision.deletes());
        assert!(decision.selected().is_empty());
    }

    #[test]
    fn critical_level_still_respects_every_guard() {
        let mut config = config();
        config.auto_mode = AutoCleanMode::AutoApproved;
        let entries = [
            rule_entry("approved", GB, true),
            rule_entry("unapproved", GB, false),
        ];
        let decision = evaluate(
            &config,
            &sample(1, 1000), // critical
            &entries,
            Some(Path::new("/Users/me")),
            None,
            None,
            0,
        );
        assert_eq!(decision.level, AlertLevel::Critical);
        assert_eq!(
            decision.selected().len(),
            1,
            "urgency does not widen the rules"
        );
    }

    #[test]
    fn still_matches_rejects_a_changed_object() {
        let base = std::env::temp_dir().join(format!("sift-monitor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let target = base.join("cache");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f"), vec![0u8; 10]).unwrap();

        let meta = std::fs::symlink_metadata(&target).unwrap();
        let good = CleanableEntry {
            fingerprint: PathFingerprint::new(
                NodeKey::from_path(&target),
                meta.len(),
                crate::policy::modified_ms(&meta),
                true,
            ),
            path: target.clone(),
            display_path: "~/cache".into(),
            name: "cache".into(),
            kind_token: "rebuildableCache".into(),
            size: 10,
            safety: Safety::Safe,
            confidence: 0.9,
            reason: Reason::key("k"),
            source: VerdictSource::rule("r"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: true,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: Reason::key("k"),
        };
        assert!(still_matches(&good), "an unchanged directory must verify");

        // A different modification time no longer matches the approval.
        let mut changed = good.clone();
        changed.fingerprint.mtime_ms = changed.fingerprint.mtime_ms.saturating_sub(5_000);
        assert!(!still_matches(&changed));

        // A missing path never matches.
        let mut gone = good.clone();
        gone.path = base.join("vanished");
        assert!(!still_matches(&gone));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A directory's remembered size is its subtree total, so the check must
    /// not compare it against the directory's own `st_size`.
    #[test]
    fn a_directory_is_verified_by_mtime_not_by_aggregate_size() {
        let base =
            std::env::temp_dir().join(format!("sift-monitor-dircheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("node_modules");
        std::fs::create_dir_all(&target).unwrap();

        let meta = std::fs::symlink_metadata(&target).unwrap();
        let entry = CleanableEntry {
            // The aggregate (what the scan recorded), deliberately different
            // from the directory's own st_size.
            fingerprint: PathFingerprint::new(
                NodeKey::from_path(&target),
                4_000_000_000,
                modified_ms(&meta),
                true,
            ),
            path: target.clone(),
            display_path: "~/node_modules".into(),
            name: "node_modules".into(),
            kind_token: "rebuildableCache".into(),
            size: 4_000_000_000,
            safety: Safety::Safe,
            confidence: 0.9,
            reason: Reason::key("k"),
            source: VerdictSource::rule("dir.node_modules"),
            first_seen_ms: 0,
            last_seen_ms: 0,
            times_seen: 1,
            approved_for_auto: true,
        cleanup_command: None,
        cleanup_method: "trashItem".into(),
        impact: Reason::key("k"),
        };
        assert!(
            still_matches(&entry),
            "an unchanged directory must verify despite the aggregate size"
        );

        // A stale modification time must not verify.
        let mut stale = entry.clone();
        stale.fingerprint.mtime_ms = stale.fingerprint.mtime_ms.saturating_sub(5_000);
        assert!(!still_matches(&stale));

        // A file appearing where a directory was must not verify.
        let _ = std::fs::remove_dir_all(&target);
        std::fs::write(&target, b"now a file").unwrap();
        assert!(!still_matches(&entry));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn inside_home_is_prefix_based_and_refuses_without_a_home() {
        let home = Path::new("/Users/me");
        assert!(inside_home(Path::new("/Users/me/a/b"), Some(home)));
        assert!(!inside_home(Path::new("/Users/men"), Some(home)));
        assert!(!inside_home(Path::new("/Users/me"), None));
    }

    #[test]
    fn inside_home_normalizes_dot_dot_lexically() {
        let home = Path::new("/Users/me");
        // A wander out and back in still resolves inside.
        assert!(inside_home(
            Path::new("/Users/me/project/../project/file"),
            Some(home)
        ));
        // The bypass the review found: `<home>/../<outside>` is not inside.
        assert!(!inside_home(
            Path::new("/Users/me/../etc/passwd"),
            Some(home)
        ));
        assert!(!inside_home(Path::new("/Users/me/../../outside"), Some(home)));
        // `.` components are noise.
        assert!(inside_home(Path::new("/Users/me/./a"), Some(home)));
    }

    #[cfg(unix)]
    #[test]
    fn resolved_inside_home_catches_an_outward_symlink() {
        use std::fs;
        let base =
            std::env::temp_dir().join(format!("sift-home-real-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let home = base.join("home");
        let outside = base.join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        // Looks home-shaped, points out of the home.
        std::os::unix::fs::symlink(&outside, home.join("link")).unwrap();

        // The lexical check cannot see through the symlink...
        assert!(inside_home(&home.join("link"), Some(&home)));
        // ...the proven check refuses it.
        assert!(!resolved_inside_home(&home.join("link"), Some(&home)));

        // An entry genuinely inside passes; an unresolvable home refuses all.
        fs::write(home.join("real.txt"), b"x").unwrap();
        assert!(resolved_inside_home(&home.join("real.txt"), Some(&home)));
        assert!(!resolved_inside_home(
            &home.join("real.txt"),
            Some(&base.join("missing"))
        ));
        assert!(!resolved_inside_home(&home.join("real.txt"), None));
        // A path that no longer exists cannot be proven.
        assert!(!resolved_inside_home(&home.join("gone"), Some(&home)));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn config_from_settings_carries_the_thresholds() {
        let settings = MonitorSettings {
            warn_free_ratio: 0.2,
            critical_free_ratio: 0.04,
            auto_mode: AutoCleanMode::AutoApproved,
            clean_cooldown_secs: 60,
            ..MonitorSettings::default()
        };
        let config = MonitorConfig::from_settings(&settings);
        assert_eq!(config.warn_free_ratio, 0.2);
        assert_eq!(config.clean_cooldown_ms, 60_000);
        assert!(config.auto_mode.may_delete());
    }

    #[test]
    fn paths_of_preserves_order() {
        let entries = vec![rule_entry("a", 1, true), rule_entry("b", 2, true)];
        let paths = paths_of(&entries);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("a"));
        assert!(paths[1].ends_with("b"));
        let _ = ByteSize::ZERO;
    }

    #[test]
    fn filetime_epoch_anchor_is_unix_epoch() {
        // FILETIME of 1970-01-01 00:00 UTC must map to 0 Unix ms.
        assert_eq!(filetime_to_unix_ms(116_444_736_000_000_000), 0);
    }

    #[test]
    fn filetime_known_date_converts_seconds_and_fraction() {
        // 2020-01-01 00:00 UTC = Unix 1577836800 s.
        let filetime = 116_444_736_000_000_000 + 1_577_836_800 * 10_000_000;
        assert_eq!(filetime_to_unix_ms(filetime), 1_577_836_800_000);
    }
}
