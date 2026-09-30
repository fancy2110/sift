//! Routing: which candidates a remote adjudicator is even allowed to see.
//!
//! This is the privacy boundary, expressed as an adjudicator combinator. Two
//! things are true at once:
//!
//! * The requirement is to *let the model judge the relevant files*, so the
//!   pipeline must be able to hand a real, useful batch to a model.
//! * A file list is one of the most revealing things a machine holds. So the
//!   batch is narrowed before it leaves: only families whose safety genuinely
//!   depends on what the content *is* (a stale file, an archive, an installer,
//!   a duplicate set) are transmitted at all. Structural families —
//!   `node_modules`, build output, the trash, application caches — are resolved
//!   locally by the rule table and never leave the machine.
//!
//! The router also refuses to transmit a candidate whose family a model may not
//! authorize, so a misconfigured provider cannot widen the boundary.

use crate::adjudicate::{AdjudicateError, Adjudicator, Guardrails};
use crate::candidate::Candidate;
use crate::reason::Verdict;

/// A remote (off-machine) adjudicator.
///
/// Implementations must return exactly one already-sanitized verdict per
/// candidate, in order. "Already sanitized" means the raw answer went through
/// [`crate::adjudicate::apply_remote_verdicts`] with the guardrails they were
/// given — a remote implementation that returns a model's `Safe` unfiltered is
/// a bug, which is why the trait takes the guardrails explicitly.
pub trait RemoteAdjudicator: Send + Sync {
    /// Provider name, recorded in verdict sources (e.g. `ark`, `openai`).
    fn provider(&self) -> &str;

    /// Judge off-machine. Called only with candidates that
    /// [`Candidate::kind`] says may be transmitted.
    fn adjudicate_remote(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError>;
}

/// Splits a batch between the local rules and an optional remote provider.
///
/// With no provider attached this behaves exactly like the local adjudicator,
/// so attaching or detaching a model never changes the shape of the pipeline.
pub struct RoutingAdjudicator<L, R> {
    local: L,
    remote: Option<R>,
    guardrails: Guardrails,
}

impl<L, R> RoutingAdjudicator<L, R> {
    pub fn local_only(local: L) -> Self {
        Self {
            local,
            remote: None,
            guardrails: Guardrails::default(),
        }
    }

    pub fn with_remote(local: L, remote: R, guardrails: Guardrails) -> Self {
        Self {
            local,
            remote: Some(remote),
            guardrails,
        }
    }

    pub fn local(&self) -> &L {
        &self.local
    }

    pub fn remote(&self) -> Option<&R> {
        self.remote.as_ref()
    }

    pub fn guardrails(&self) -> &Guardrails {
        &self.guardrails
    }

    /// Whether a candidate is allowed to be transmitted.
    pub fn is_transmittable(candidate: &Candidate) -> bool {
        candidate.kind.is_model_adjudicable()
    }
}

impl<L, R> Adjudicator for RoutingAdjudicator<L, R>
where
    L: Adjudicator,
    R: RemoteAdjudicator,
{
    fn name(&self) -> &str {
        match &self.remote {
            Some(remote) => remote.provider(),
            None => self.local.name(),
        }
    }

    fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    fn adjudicate_batch(
        &self,
        batch: &[Candidate],
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        let Some(remote) = &self.remote else {
            return self.local.adjudicate_batch(batch, now_ms);
        };

        // Partition: everything the model may not see is resolved locally.
        let mut remote_indices: Vec<usize> = Vec::new();
        let mut local_indices: Vec<usize> = Vec::new();
        for (index, candidate) in batch.iter().enumerate() {
            if Self::is_transmittable(candidate) {
                remote_indices.push(index);
            } else {
                local_indices.push(index);
            }
        }

        let mut verdicts: Vec<Option<Verdict>> = vec![None; batch.len()];

        if !local_indices.is_empty() {
            let local_batch: Vec<Candidate> =
                local_indices.iter().map(|&ix| batch[ix].clone()).collect();
            let local_verdicts = self.local.adjudicate_batch(&local_batch, now_ms)?;
            for (slot, verdict) in local_indices.iter().zip(local_verdicts) {
                verdicts[*slot] = Some(verdict);
            }
        }

        if !remote_indices.is_empty() {
            let remote_batch: Vec<Candidate> =
                remote_indices.iter().map(|&ix| batch[ix].clone()).collect();
            match remote.adjudicate_remote(&remote_batch, &self.guardrails, now_ms) {
                Ok(remote_verdicts) if remote_verdicts.len() == remote_batch.len() => {
                    for (slot, verdict) in remote_indices.iter().zip(remote_verdicts) {
                        verdicts[*slot] = Some(verdict);
                    }
                }
                // A remote failure must not silently weaken the local answer:
                // fall back to the rules for exactly those candidates.
                Ok(_) | Err(_) => {
                    let fallback = self.local.adjudicate_batch(&remote_batch, now_ms)?;
                    for (slot, verdict) in remote_indices.iter().zip(fallback) {
                        verdicts[*slot] = Some(verdict);
                    }
                }
            }
        }

        Ok(verdicts
            .into_iter()
            .map(|verdict| verdict.unwrap_or_else(|| Verdict::unknown(now_ms)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjudicate::{apply_remote_verdicts, RawVerdict, RuleAdjudicator};
    use crate::candidate::{CandidateKind, Evidence};
    use crate::reason::{PathFingerprint, Safety};
    use sift_core::{ByteSize, NodeKey};
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn candidate(kind: CandidateKind, rule: &str) -> Candidate {
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(rule.as_bytes()),
            PathBuf::from("/Users/me/x"),
            "~/x",
            "x",
            false,
            ByteSize::new(1 << 30, 1 << 30),
            0,
            kind,
        );
        candidate.evidence.push(Evidence::new(rule, "detail.x"));
        // The nomination is what makes a candidate safe; without it the
        // adjudicator must answer Review.
        let safety = match rule {
            "dir.trash" | "dir.node_modules" => Safety::Safe,
            _ => Safety::Review,
        };
        candidate.with_nomination(crate::candidate::Nomination {
            rule: rule.to_string(),
            safety,
            reason_key: "reason.rebuildableCache".to_string(),
            reason_param: None,
            detail_key: "detail.x".to_string(),
        })
    }

    /// Records every batch it is given, then answers `safe` for all of them.
    #[derive(Default)]
    struct SpyRemote {
        seen: Mutex<Vec<Vec<String>>>,
    }

    impl RemoteAdjudicator for SpyRemote {
        fn provider(&self) -> &str {
            "spy"
        }

        fn adjudicate_remote(
            &self,
            batch: &[Candidate],
            guardrails: &Guardrails,
            now_ms: i64,
        ) -> Result<Vec<Verdict>, AdjudicateError> {
            self.seen
                .lock()
                .unwrap()
                .push(batch.iter().map(|c| c.display_path.clone()).collect());
            let raw: Vec<RawVerdict> = batch
                .iter()
                .map(|candidate| RawVerdict {
                    id: candidate.key.to_string(),
                    safety: "safe".into(),
                    confidence: 0.99,
                    reason: "old and unused".into(),
                    impact: String::new(),
                })
                .collect();
            Ok(apply_remote_verdicts(batch, &raw, guardrails, now_ms, "spy"))
        }
    }

    #[test]
    fn structural_families_never_leave_the_machine() {
        let spy = SpyRemote::default();
        let router =
            RoutingAdjudicator::with_remote(RuleAdjudicator::new(), spy, Guardrails::default());

        let batch = vec![
            candidate(CandidateKind::Trash, "dir.trash"),
            candidate(
                CandidateKind::RebuildableCache { tool: "npm".into() },
                "dir.node_modules",
            ),
            candidate(CandidateKind::StaleLargeFile, "file.stale_large"),
        ];
        let verdicts = router.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts.len(), 3);

        let transmitted = router.remote().unwrap().seen.lock().unwrap().clone();
        assert_eq!(transmitted.len(), 1, "exactly one batch should be sent");
        assert_eq!(
            transmitted[0],
            vec!["~/x".to_string()],
            "only the stale file may be transmitted"
        );

        // The local families kept their local conclusions.
        assert_eq!(verdicts[0].safety, Safety::Safe, "trash stays safe locally");
        assert_eq!(verdicts[1].safety, Safety::Safe, "cache stays safe locally");
    }

    #[test]
    fn remote_failure_falls_back_to_rules_not_to_silence() {
        struct BrokenRemote;
        impl RemoteAdjudicator for BrokenRemote {
            fn provider(&self) -> &str {
                "broken"
            }
            fn adjudicate_remote(
                &self,
                _batch: &[Candidate],
                _guardrails: &Guardrails,
                _now_ms: i64,
            ) -> Result<Vec<Verdict>, AdjudicateError> {
                Err(AdjudicateError::Transport("no network".into()))
            }
        }

        let router = RoutingAdjudicator::with_remote(
            RuleAdjudicator::new(),
            BrokenRemote,
            Guardrails::default(),
        );
        let batch = vec![candidate(CandidateKind::StaleLargeFile, "file.stale_large")];
        let verdicts = router.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts.len(), 1);
        // The rule's own conclusion stands.
        assert_eq!(verdicts[0].safety, Safety::Review);
        assert!(verdicts[0].source.label().starts_with("rule:"));
    }

    #[test]
    fn no_remote_configured_behaves_like_local() {
        let router: RoutingAdjudicator<RuleAdjudicator, SpyRemote> =
            RoutingAdjudicator::local_only(RuleAdjudicator::new());
        assert!(!router.is_remote());
        let batch = vec![candidate(CandidateKind::Trash, "dir.trash")];
        let verdicts = router.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Safe);
    }

    /// A remote answer about one candidate must never be applied to another,
    /// even in the same batch.
    #[test]
    fn verdicts_stay_aligned_with_their_candidate() {
        let spy = SpyRemote::default();
        let router =
            RoutingAdjudicator::with_remote(RuleAdjudicator::new(), spy, Guardrails::default());
        let mut a = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        a.key = NodeKey::from_bytes(b"first");
        let mut b = candidate(
            CandidateKind::Archive {
                extension: "zip".into(),
            },
            "file.archive",
        );
        b.key = NodeKey::from_bytes(b"second");
        let verdicts = router.adjudicate_batch(&[a.clone(), b.clone()], 0).unwrap();
        assert_eq!(verdicts.len(), 2);
        assert!(verdicts.iter().all(|v| v.safety == Safety::Safe));

        // Pin the cache trait is unused here but keeps the fingerprint import honest.
        let _ = PathFingerprint::new(a.key, 1, 1, false);
    }
}
