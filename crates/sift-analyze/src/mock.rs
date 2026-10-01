//! Offline mock adjudicator.
//!
//! Used whenever no API token is configured. It runs the same prompt builder
//! and output contract a real provider uses, so the prompt stays reusable and
//! testable, but answers are produced locally from candidate metadata.
//!
//! Safety policy mirrors the rules: only structurally removable families —
//! rebuildable caches, cache directories, trash, logs, temp files — are
//! affirmed. Content-dependent families (`StaleLargeFile`, archives,
//! duplicates) stay `Review`, because judging those needs a real model.

use std::time::Duration;

use crate::adjudicate::{apply_remote_verdicts, AdjudicateError, Guardrails, RawVerdict};
use crate::candidate::{Candidate, CandidateKind};
use crate::cleanup_plan::{CleanupCommand, CleanupImpact};
use crate::reason::{Reason, Safety, Verdict, VerdictSource};
use crate::prompt::build_prompt;
use crate::route::RemoteAdjudicator;

/// Local stand-in for a cloud model.
#[derive(Debug, Clone)]
pub struct MockAdjudicator {
    label: String,
    language: String,
    latency: Duration,
}

impl MockAdjudicator {
    pub fn new() -> Self {
        Self {
            label: "offline-mock".into(),
            language: "en".into(),
            latency: Duration::from_millis(120),
        }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Whether the mock can affirm safety locally from the pattern alone.
    fn is_structurally_safe(kind: &CandidateKind) -> bool {
        matches!(
            kind,
            CandidateKind::RebuildableCache { .. }
                | CandidateKind::CacheDirectory { .. }
                | CandidateKind::Trash
                | CandidateKind::Log { .. }
                | CandidateKind::TempFile
        )
    }

    /// Local stand-in answer for one candidate, expressed as a [`RawVerdict`]
    /// so the same guardrail merge as a real provider applies.
    fn raw_for(&self, candidate: &Candidate) -> RawVerdict {
        let safe = Self::is_structurally_safe(&candidate.kind);
        let safety = if safe { "safe" } else { "review" };
        RawVerdict {
            id: candidate.key.to_string(),
            safety: safety.into(),
            confidence: if safe { 0.86 } else { 0.55 },
            reason: self.reason_text(candidate),
            impact: self.impact_for(candidate),
            commands: self.commands_for(candidate),
        }
    }

    fn reason_text(&self, candidate: &Candidate) -> String {
        match &candidate.kind {
            CandidateKind::RebuildableCache { .. } | CandidateKind::CacheDirectory { .. } => {
                format!("{} is regenerated automatically when needed.", candidate.name)
            }
            CandidateKind::Trash => "Items in the Trash are already marked for removal.".into(),
            CandidateKind::Log { .. } | CandidateKind::TempFile => {
                format!("{} is a temporary diagnostic file.", candidate.name)
            }
            other => format!(
                "{} needs a human decision; the offline mock cannot classify its content.",
                other.label_key()
            ),
        }
    }

    fn impact_for(&self, candidate: &Candidate) -> Option<CleanupImpact> {
        let reversible = Self::is_structurally_safe(&candidate.kind);
        let summary = format!(
            "{}: removing it frees the reported {} bytes.",
            candidate.display_path,
            candidate.size.dominant()
        );
        CleanupImpact::new(summary, reversible).sanitized()
    }

    fn commands_for(&self, candidate: &Candidate) -> Vec<CleanupCommand> {
        // A universally correct, object-scoped native action only exists for
        // the user Trash. Everything else goes through the app's dry-run flow.
        if matches!(candidate.kind, CandidateKind::Trash) && cfg!(target_os = "macos") {
            if let Some(command) =
                CleanupCommand::new("rm -rf ~/.Trash/*", "macos", "Empty the user Trash.")
                    .sanitized()
            {
                return vec![command];
            }
        }
        Vec::new()
    }
}

impl Default for MockAdjudicator {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteAdjudicator for MockAdjudicator {
    fn provider(&self) -> &str {
        &self.label
    }

    fn adjudicate_remote(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        std::thread::sleep(self.latency);
        // Validate the shared prompt even offline (prompts must keep building).
        let _ = build_prompt(batch, &self.language);

        let mut verdicts = Vec::with_capacity(batch.len());
        // Build raw answers, then partition: structurally safe families are a
        // local conclusion and must NOT pass through the guardrail that
        // downgrades remote structural answers; content families still merge
        // exactly as a real provider would.
        let raw: Vec<RawVerdict> = batch.iter().map(|candidate| self.raw_for(candidate)).collect();
        for (candidate, raw) in batch.iter().zip(raw) {
            if Self::is_structurally_safe(&candidate.kind) {
                verdicts.push(Verdict {
                    safety: Safety::Safe,
                    confidence: raw.confidence,
                    reason: Reason::text(raw.reason),
                    judged_at_ms: now_ms,
                    source: VerdictSource::Mock {
                        label: self.label.clone(),
                    },
                    impact: raw.impact,
                    commands: raw.commands,
                });
            } else {
                verdicts.extend(
                    apply_remote_verdicts(
                        std::slice::from_ref(candidate),
                        std::slice::from_ref(&raw),
                        guardrails,
                        now_ms,
                    )
                    .into_iter()
                    .map(|mut verdict| {
                        verdict.source = VerdictSource::Mock {
                            label: self.label.clone(),
                        };
                        verdict
                    }),
                );
            }
        }
        Ok(verdicts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::{ByteSize, NodeKey};

    fn candidate(kind: CandidateKind, name: &str) -> Candidate {
        // Mirror the rule pipeline: structural families carry a Safe
        // nomination; content families carry Review.
        let safety = match kind {
            CandidateKind::RebuildableCache { .. }
            | CandidateKind::CacheDirectory { .. }
            | CandidateKind::Trash
            | CandidateKind::Log { .. }
            | CandidateKind::TempFile => Safety::Safe,
            CandidateKind::PackageInstaller { .. }
            | CandidateKind::Archive { .. }
            | CandidateKind::StaleLargeFile
            | CandidateKind::DuplicateGroup { .. }
            | CandidateKind::UserMarked => Safety::Review,
        };
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(name.as_bytes()),
            std::path::PathBuf::from(format!("/Users/me/{name}")),
            &format!("~/{name}"),
            name,
            false,
            ByteSize::new(1_000_000, 4_096),
            0,
            kind,
        );
        candidate.with_nomination(crate::candidate::Nomination {
            rule: "mock.test".to_string(),
            safety,
            reason_key: "reason.test".to_string(),
            reason_param: None,
            detail_key: "detail.test".to_string(),
        })
    }

    #[test]
    fn structural_caches_are_safe_but_content_families_are_review() {
        let mock = MockAdjudicator::new().with_latency(Duration::ZERO);
        let batch = vec![
            candidate(CandidateKind::RebuildableCache { tool: "npm".into() }, "node_modules"),
            candidate(CandidateKind::Trash, "Trash"),
            candidate(CandidateKind::StaleLargeFile, "old.bin"),
            candidate(CandidateKind::Archive { extension: "zip".into() }, "a.zip"),
        ];
        let verdicts = mock
            .adjudicate_remote(&batch, &Guardrails::default(), 0)
            .unwrap();
        assert_eq!(verdicts.len(), 4);
        assert_eq!(verdicts[0].safety, Safety::Safe);
        assert_eq!(verdicts[1].safety, Safety::Safe);
        assert_eq!(verdicts[2].safety, Safety::Review);
        assert_eq!(verdicts[3].safety, Safety::Review);
    }

    #[test]
    fn safe_verdicts_carry_impact_and_trash_carries_a_command_on_macos() {
        let mock = MockAdjudicator::new().with_latency(Duration::ZERO);
        let batch = vec![candidate(CandidateKind::Trash, "Trash")];
        let verdicts = mock
            .adjudicate_remote(&batch, &Guardrails::default(), 0)
            .unwrap();
        assert!(verdicts[0].impact.is_some());
        assert_eq!(verdicts[0].source, VerdictSource::Mock { label: "offline-mock".into() });
        if cfg!(target_os = "macos") {
            assert_eq!(verdicts[0].commands.len(), 1);
        }
    }

    #[test]
    fn empty_batch_is_an_ok_empty_vec() {
        let mock = MockAdjudicator::new();
        assert!(mock
            .adjudicate_remote(&[], &Guardrails::default(), 0)
            .unwrap()
            .is_empty());
    }
}
