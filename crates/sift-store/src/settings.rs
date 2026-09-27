//! User preferences, including the monitor's trigger thresholds.
//!
//! Deliberately credential-free. The analysis configuration names the
//! *environment variable* an API key should be read from
//! ([`AiSettings::api_key_env`]) instead of storing the key itself: a disk
//! cleaner that writes a bearer token into a world-readable JSON file next to
//! the file list it is analyzing would be a poor trade.

use serde::{Deserialize, Serialize};

/// What the background monitor should do when space runs low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub enum AutoCleanMode {
    /// Watch, but say nothing.
    Off,
    /// Tell the user when the threshold is crossed; never act.
    #[default]
    NotifyOnly,
    /// Remove entries the user has explicitly approved, without asking.
    AutoApproved,
}

impl AutoCleanMode {
    pub const fn may_delete(self) -> bool {
        matches!(self, AutoCleanMode::AutoApproved)
    }
}

/// Background disk-space monitoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSettings {
    pub enabled: bool,
    /// How often the free-space probe runs, in seconds.
    pub interval_secs: u64,
    /// Warn when free space falls below this fraction of the volume.
    pub warn_free_ratio: f64,
    /// Escalate when free space falls below this fraction.
    pub critical_free_ratio: f64,
    /// Absolute floor: warn regardless of ratio when free space is below this.
    pub min_free_bytes: u64,
    pub auto_mode: AutoCleanMode,
    /// Minimum time between two unattended cleanups.
    pub clean_cooldown_secs: u64,
    /// A single unattended cleanup may not exceed this. Larger batches need the
    /// user's confirmation, because "the app deleted 200 GB while I slept" is
    /// not a recoverable surprise.
    pub max_auto_clean_bytes: u64,
    /// Only act on paths inside the user's home directory. Nothing outside it is
    /// ever removed without an explicit confirmation.
    pub home_only: bool,
    /// Approve structural, rule-sourced findings for unattended cleanup as soon
    /// as they are discovered. Off by default: convenience must be asked for.
    pub auto_approve_structural: bool,
    /// Run the engine daily and clean remembered safe items unattended.
    #[serde(default = "default_scheduled_cleanup")]
    pub scheduled_cleanup_enabled: bool,
    /// Local hour the daily cleanup runs at.
    #[serde(default = "default_scheduled_hour")]
    pub scheduled_hour: u32,
    /// How long a notification stays relevant before the monitor may repeat it.
    pub notify_cooldown_secs: u64,
}

fn default_scheduled_cleanup() -> bool {
    true
}

fn default_scheduled_hour() -> u32 {
    4
}

impl Default for MonitorSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            warn_free_ratio: 0.15,
            critical_free_ratio: 0.05,
            min_free_bytes: 10 * 1024 * 1024 * 1024,
            auto_mode: AutoCleanMode::default(),
            clean_cooldown_secs: 3600,
            max_auto_clean_bytes: 20 * 1024 * 1024 * 1024,
            home_only: true,
            auto_approve_structural: false,
            scheduled_cleanup_enabled: true,
            scheduled_hour: 4,
            notify_cooldown_secs: 1800,
        }
    }
}

impl MonitorSettings {
    /// Clamp a loaded configuration into a sane shape.
    ///
    /// A settings file can be hand-edited or written by an older version; a
    /// nonsensical threshold must not make the monitor fire constantly or never.
    pub fn normalized(mut self) -> Self {
        self.interval_secs = self.interval_secs.clamp(30, 24 * 3600);
        self.notify_cooldown_secs = self.notify_cooldown_secs.min(24 * 3600);
        self.clean_cooldown_secs = self.clean_cooldown_secs.clamp(60, 7 * 24 * 3600);
        // Ratios must be ordered and inside the unit interval.
        self.warn_free_ratio = clamp_ratio(self.warn_free_ratio, 0.15);
        self.critical_free_ratio = clamp_ratio(self.critical_free_ratio, 0.05);
        if self.critical_free_ratio >= self.warn_free_ratio {
            // The critical line must sit below the warning line, or the state
            // machine can never reach a warning without also being critical.
            self.critical_free_ratio = (self.warn_free_ratio / 2.0).max(0.001);
        }
        self.scheduled_hour = self.scheduled_hour.clamp(0, 23);
        self
    }
}

fn clamp_ratio(value: f64, fallback: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 || value > 1.0 {
        fallback
    } else {
        value
    }
}

/// How much analysis to run, and how aggressive it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisSettings {
    /// Candidates below this are not worth the user's attention.
    pub min_candidate_bytes: u64,
    /// Whether to enumerate the usual download directories for file-level
    /// candidates in addition to the directory pass.
    pub include_files: bool,
    /// Days without modification before a large file counts as stale.
    pub stale_days: u64,
    /// Cap on candidates handed to adjudication.
    pub max_candidates: usize,
    /// Reuse remembered verdicts instead of re-judging. Turning this off is
    /// mostly useful when debugging a rule change.
    pub use_verdict_cache: bool,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            min_candidate_bytes: 8 * 1024 * 1024,
            include_files: true,
            stale_days: 180,
            max_candidates: 400,
            use_verdict_cache: true,
        }
    }
}

impl AnalysisSettings {
    pub fn normalized(mut self) -> Self {
        self.min_candidate_bytes = self.min_candidate_bytes.max(1024);
        self.stale_days = self.stale_days.clamp(1, 3650);
        self.max_candidates = self.max_candidates.clamp(10, 5_000);
        self
    }
}

/// Remote (language-model) adjudication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    /// Whether the user has turned model adjudication on at all.
    pub enabled: bool,
    /// Whether the user has agreed to send file *metadata* off the machine.
    /// Kept separate from `enabled` so the consent step is explicit and can be
    /// revoked without losing the endpoint configuration.
    pub consent_granted: bool,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    /// Name of the environment variable holding the API key. The key itself is
    /// never written to disk by this crate.
    pub api_key_env: String,
    /// Language model reasons should be written in.
    pub language: String,
    /// Candidates per request.
    pub batch_size: usize,
    /// Only send candidates the family rules allow, and never send a candidate
    /// whose path is outside the home directory. On by default; turning it off
    /// is not supported, which is why there is no setter.
    pub redact_paths: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            consent_granted: false,
            provider: String::new(),
            endpoint: String::new(),
            model: String::new(),
            api_key_env: "SIFT_AI_API_KEY".to_string(),
            language: "zh".to_string(),
            batch_size: 30,
            redact_paths: true,
        }
    }
}

impl AiSettings {
    /// Whether a remote request may be made right now.
    pub fn is_usable(&self) -> bool {
        self.enabled
            && self.consent_granted
            && !self.endpoint.trim().is_empty()
            && !self.model.trim().is_empty()
    }

    /// Read the API key from the environment, if configured.
    pub fn api_key_from_env(&self) -> Option<String> {
        let name = self.api_key_env.trim();
        if name.is_empty() {
            return None;
        }
        std::env::var(name)
            .ok()
            .filter(|key| !key.trim().is_empty())
    }
}

/// Everything the product persists about the user's preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// BCP-47 tag for the interface.
    pub language: String,
    pub monitor: MonitorSettings,
    pub analysis: AnalysisSettings,
    pub ai: AiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "zh".to_string(),
            monitor: MonitorSettings::default(),
            analysis: AnalysisSettings::default(),
            ai: AiSettings::default(),
        }
    }
}

impl Settings {
    /// Repair a loaded configuration.
    pub fn normalized(self) -> Self {
        Self {
            monitor: self.monitor.normalized(),
            analysis: self.analysis.normalized(),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_do_not_delete_without_being_asked() {
        let settings = Settings::default();
        assert!(!settings.monitor.auto_mode.may_delete());
        assert!(!settings.monitor.auto_approve_structural);
        assert!(settings.monitor.home_only);
        assert!(!settings.ai.enabled);
        assert!(!settings.ai.consent_granted);
        assert!(settings.ai.redact_paths);
    }

    #[test]
    fn absurd_thresholds_are_repaired() {
        let broken = MonitorSettings {
            interval_secs: 0,
            warn_free_ratio: f64::NAN,
            critical_free_ratio: 5.0,
            min_free_bytes: u64::MAX,
            ..MonitorSettings::default()
        }
        .normalized();
        assert!(broken.interval_secs >= 30);
        assert!(broken.warn_free_ratio > 0.0 && broken.warn_free_ratio <= 1.0);
        assert!(broken.critical_free_ratio < broken.warn_free_ratio);
    }

    #[test]
    fn an_inverted_threshold_pair_is_repaired() {
        let settings = MonitorSettings {
            warn_free_ratio: 0.1,
            critical_free_ratio: 0.9,
            ..MonitorSettings::default()
        }
        .normalized();
        assert!(
            settings.critical_free_ratio < settings.warn_free_ratio,
            "critical {} must sit below warn {}",
            settings.critical_free_ratio,
            settings.warn_free_ratio
        );
    }

    #[test]
    fn equal_thresholds_are_separated() {
        let settings = MonitorSettings {
            warn_free_ratio: 0.1,
            critical_free_ratio: 0.1,
            ..MonitorSettings::default()
        }
        .normalized();
        assert!(settings.critical_free_ratio < settings.warn_free_ratio);
    }

    #[test]
    fn analysis_limits_are_clamped() {
        let analysis = AnalysisSettings {
            min_candidate_bytes: 0,
            stale_days: 0,
            max_candidates: usize::MAX,
            ..AnalysisSettings::default()
        }
        .normalized();
        assert_eq!(analysis.min_candidate_bytes, 1024);
        assert_eq!(analysis.stale_days, 1);
        assert!(analysis.max_candidates <= 5_000);
    }

    #[test]
    fn remote_use_requires_both_enablement_and_consent() {
        let mut ai = AiSettings {
            enabled: true,
            endpoint: "https://example.test/v1/chat".into(),
            model: "m".into(),
            ..AiSettings::default()
        };
        assert!(!ai.is_usable(), "no consent");
        ai.consent_granted = true;
        assert!(ai.is_usable());

        // An endpoint without a model is not usable either.
        ai.model = String::new();
        assert!(!ai.is_usable());
    }

    #[test]
    fn the_api_key_comes_from_the_environment_not_the_file() {
        let ai = AiSettings {
            api_key_env: "SIFT_TEST_KEY_UNSET_12345".into(),
            ..AiSettings::default()
        };
        assert!(ai.api_key_from_env().is_none());

        // And the serialized settings must never contain a secret field.
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains("apiKeyEnv"));
        assert!(!json.contains("apiKey\""), "no key field may be persisted");
        assert!(!json.contains("api_key"));
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            language: "en".into(),
            monitor: MonitorSettings {
                auto_mode: AutoCleanMode::AutoApproved,
                auto_approve_structural: true,
                ..MonitorSettings::default()
            },
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, back);
        assert!(back.monitor.auto_mode.may_delete());
    }
}
