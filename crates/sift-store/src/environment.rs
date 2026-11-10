//! Persisted environment snapshot and maintenance hints.
//!
//! The document is deliberately separate from the scan journal: it is small,
//! rewritten at most once per analysis, and must survive a crash without
//! touching the hot scan path.

use serde::{Deserialize, Serialize};
use sift_apps::{HintStatus, MaintenanceHint, ScanContext};

/// Bump when the persisted shape changes; an older document is archived like
/// every other store document.
pub const ENVIRONMENT_SCHEMA_VERSION: u32 = 1;

const MAX_HINTS: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDoc {
    pub schema_version: u32,
    pub updated_at_ms: i64,
    pub context: ScanContext,
    pub hints: Vec<MaintenanceHint>,
}

impl Default for EnvironmentDoc {
    fn default() -> Self {
        Self {
            schema_version: ENVIRONMENT_SCHEMA_VERSION,
            updated_at_ms: 0,
            context: ScanContext::default(),
            hints: Vec::new(),
        }
    }
}

impl EnvironmentDoc {
    /// Replace the snapshot and merge hints by id.
    ///
    /// - A validated/rejected hint keeps its state across rescans; only its
    ///   `last_seen` and version are refreshed.
    /// - A hint that disappears becomes `stale` instead of vanishing, so the
    ///   UI can explain why it is no longer recommended.
    pub fn record(&mut self, context: ScanContext, hints: Vec<MaintenanceHint>, now_ms: i64) {
        let incoming: std::collections::HashMap<&str, &MaintenanceHint> =
            hints.iter().map(|hint| (hint.id.as_str(), hint)).collect();

        for existing in &mut self.hints {
            match incoming.get(existing.id.as_str()) {
                Some(fresh) => {
                    existing.last_seen_ms = fresh.last_seen_ms;
                    existing.confidence = fresh.confidence;
                    existing.pm_version = fresh.pm_version.clone();
                    existing.reason_key = fresh.reason_key.clone();
                    existing.impact_key = fresh.impact_key.clone();
                }
                None => {
                    if matches!(existing.status, HintStatus::Proposed) {
                        existing.status = HintStatus::Stale;
                    }
                }
            }
        }

        let known: std::collections::HashSet<String> =
            self.hints.iter().map(|hint| hint.id.clone()).collect();
        for hint in hints {
            if !known.contains(&hint.id) {
                self.hints.push(hint);
            }
        }
        if self.hints.len() > MAX_HINTS {
            self.hints.sort_by(|a, b| {
                b.last_seen_ms
                    .cmp(&a.last_seen_ms)
                    .then_with(|| a.id.cmp(&b.id))
            });
            self.hints.truncate(MAX_HINTS);
        }

        self.context = context;
        self.updated_at_ms = now_ms;
        self.schema_version = ENVIRONMENT_SCHEMA_VERSION;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_apps::{HintSource, MaintenanceHint, ACTION_CLEAN_CACHE};

    fn hint(id: &str, status: HintStatus) -> MaintenanceHint {
        MaintenanceHint {
            id: id.to_string(),
            subject: "pm:pnpm".to_string(),
            action: ACTION_CLEAN_CACHE.to_string(),
            status,
            source: HintSource::Catalog,
            confidence: 0.6,
            reason_key: "reason".to_string(),
            impact_key: "impact".to_string(),
            model_reason: None,
            requires_admin: false,
            pm_version: Some("1.0".to_string()),
            created_at_ms: 1,
            last_seen_ms: 1,
        }
    }

    #[test]
    fn validated_hints_survive_a_rescan() {
        let mut doc = EnvironmentDoc::default();
        doc.record(
            ScanContext::default(),
            vec![hint("a", HintStatus::Validated)],
            10,
        );
        doc.record(
            ScanContext::default(),
            vec![hint("a", HintStatus::Proposed)],
            20,
        );
        assert_eq!(doc.hints.len(), 1);
        assert_eq!(doc.hints[0].status, HintStatus::Validated);
        assert_eq!(doc.hints[0].last_seen_ms, 1);
    }

    #[test]
    fn a_vanished_proposal_becomes_stale() {
        let mut doc = EnvironmentDoc::default();
        doc.record(
            ScanContext::default(),
            vec![hint("a", HintStatus::Proposed)],
            10,
        );
        doc.record(ScanContext::default(), vec![], 20);
        assert_eq!(doc.hints[0].status, HintStatus::Stale);
    }
}
