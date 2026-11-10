//! Maintenance suggestions derived from the environment snapshot.
//!
//! A hint is *data*, never a command: it names a closed action token and the
//! capability catalog decides whether/how to execute it. Model-produced hints
//! enter with the same shape and start as `proposed`; only user confirmation or
//! a successful run moves them forward.

use serde::{Deserialize, Serialize};

use crate::context::{PmFact, ScanContext};

/// Closed action vocabulary. The package-manager capability catalog maps each
/// token to a concrete, safe command template.
pub const ACTION_CLEAN_CACHE: &str = "cleanCache";
pub const ACTION_PRUNE_STORE: &str = "pruneStore";
pub const ACTION_DEDUPE: &str = "dedupe";
pub const ACTION_REMOVE_ORPHAN: &str = "removeOrphan";
pub const ACTION_REMOVE_OLD_VERSION: &str = "removeOldVersion";
pub const ACTION_VERIFY_INTEGRITY: &str = "verifyIntegrity";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HintStatus {
    Proposed,
    Validated,
    Rejected,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HintSource {
    Catalog,
    Rule,
    Model,
    User,
}

/// One maintenance suggestion attached to a package manager, app or project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceHint {
    pub id: String,
    /// `pm:pnpm`, `app:com.example`, `project:~/code/app`.
    pub subject: String,
    pub action: String,
    pub status: HintStatus,
    pub source: HintSource,
    pub confidence: f32,
    /// i18n key for a local hint; model text is stored in `model_reason`.
    pub reason_key: String,
    pub impact_key: String,
    /// Model-provided sentence, if any. Local hints leave this empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_impact: Option<String>,
    pub requires_admin: bool,
    pub pm_version: Option<String>,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
    #[serde(default)]
    pub success_count: u32,
    #[serde(default)]
    pub failure_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_validated_ms: Option<i64>,
}

/// Deterministic hints for the detected package managers. Conservative: one or
/// two per manager, all `proposed`, all requiring a user decision.
pub fn hints_for(context: &ScanContext) -> Vec<MaintenanceHint> {
    let mut out = Vec::new();
    for pm in &context.package_managers {
        if let Some(hint) = hint_for(pm, context.scanned_at_ms) {
            out.push(hint);
        }
    }
    out.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.action.cmp(&b.action)));
    out
}

fn hint_for(pm: &PmFact, now: i64) -> Option<MaintenanceHint> {
    let action = pick_action(pm)?;
    let (reason_key, impact_key, confidence) = match action {
        ACTION_PRUNE_STORE => (
            "maintenance.pm.pruneStore.reason",
            "maintenance.pm.pruneStore.impact",
            0.7,
        ),
        ACTION_CLEAN_CACHE => (
            "maintenance.pm.cleanCache.reason",
            "maintenance.pm.cleanCache.impact",
            0.6,
        ),
        ACTION_REMOVE_OLD_VERSION => (
            "maintenance.pm.removeOldVersion.reason",
            "maintenance.pm.removeOldVersion.impact",
            0.5,
        ),
        ACTION_REMOVE_ORPHAN => (
            "maintenance.pm.removeOrphan.reason",
            "maintenance.pm.removeOrphan.impact",
            0.6,
        ),
        ACTION_VERIFY_INTEGRITY => (
            "maintenance.pm.verifyIntegrity.reason",
            "maintenance.pm.verifyIntegrity.impact",
            0.4,
        ),
        _ => return None,
    };
    Some(MaintenanceHint {
        id: format!("pm:{}:{}", pm.id, action),
        subject: format!("pm:{}", pm.id),
        action: action.to_string(),
        status: HintStatus::Proposed,
        source: HintSource::Catalog,
        confidence,
        reason_key: reason_key.to_string(),
        impact_key: impact_key.to_string(),
        model_reason: None,
        model_impact: None,
        requires_admin: matches!(pm.id.as_str(), "snap" | "flatpak"),
        pm_version: pm.version.clone(),
        created_at_ms: now,
        last_seen_ms: now,
        success_count: 0,
        failure_count: 0,
        last_validated_ms: None,
    })
}

fn pick_action(pm: &PmFact) -> Option<&'static str> {
    // Priority: a store prune is more valuable than a generic cache clean, and
    // a generic cache clean is more valuable than a read-only integrity check.
    for action in [
        ACTION_PRUNE_STORE,
        ACTION_CLEAN_CACHE,
        ACTION_REMOVE_ORPHAN,
        ACTION_REMOVE_OLD_VERSION,
        ACTION_VERIFY_INTEGRITY,
    ] {
        if pm
            .capabilities
            .iter()
            .any(|capability| capability == action)
        {
            return Some(action);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{PmFact, ScanContext};

    fn pm(id: &str, capabilities: &[&str]) -> PmFact {
        PmFact {
            id: id.to_string(),
            version: Some("1.0".into()),
            detected: true,
            roots: vec![],
            projects: vec![],
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn pnpm_prefers_store_prune() {
        let mut context = ScanContext::default();
        context
            .package_managers
            .push(pm("pnpm", &["cleanCache", "pruneStore"]));
        let hints = hints_for(&context);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].action, ACTION_PRUNE_STORE);
        assert_eq!(hints[0].subject, "pm:pnpm");
        assert_eq!(hints[0].status, HintStatus::Proposed);
    }

    #[test]
    fn no_capability_means_no_hint() {
        let mut context = ScanContext::default();
        context.package_managers.push(pm("unknown", &[]));
        assert!(hints_for(&context).is_empty());
    }

    #[test]
    fn system_package_managers_are_admin_gated() {
        let mut context = ScanContext::default();
        context.package_managers.push(pm("snap", &["cleanCache"]));
        let hints = hints_for(&context);
        assert!(hints[0].requires_admin);
    }
}
