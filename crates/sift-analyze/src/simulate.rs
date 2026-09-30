//! Offline stand-in for a remote model.
//!
//! No API token is configured in this environment, so this adjudicator plays
//! the role a provider would: it consumes exactly the same reusable prompt and
//! returns one structured verdict per candidate through the same guardrails.
//! It never accesses the network or reads file contents.
//!
//! The heuristics are deliberately simple and conservative; the point is the
//! contract (a batch in, validated verdicts out), not a fake intelligence
//! ranking. A real provider is a drop-in replacement behind the same
//! [`crate::route::RemoteAdjudicator`] trait.

use crate::adjudicate::{apply_remote_verdicts, AdjudicateError, Guardrails, RawVerdict};
use crate::candidate::{Candidate, CandidateKind};
use crate::route::RemoteAdjudicator;

/// Deterministic, offline adjudicator standing in for a model provider.
#[derive(Debug, Default, Clone)]
pub struct SimulatedAdjudicator {
    provider: String,
}

impl SimulatedAdjudicator {
    pub fn new() -> Self {
        Self {
            provider: "simulated".to_string(),
        }
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    /// Build the raw structured answer one model would return for `candidate`.
    fn raw_for(candidate: &Candidate, now_ms: i64) -> RawVerdict {
        let id = candidate.key.to_string();
        let age_days = crate::rules::age_days(candidate.mtime_ms, now_ms);
        let large = candidate.size.dominant() >= 256 * 1024 * 1024;

        // Conservative mapping. Anything ambiguous stays "review"; a model must
        // never be the sole reason something becomes automatically removable.
        let (safety, confidence) = match &candidate.kind {
            CandidateKind::PackageInstaller { .. } if age_days >= 30 => {
                ("safe", 0.86)
            }
            CandidateKind::StaleLargeFile if age_days >= 365 => ("review", 0.6),
            CandidateKind::Archive { .. } if large => ("review", 0.55),
            CandidateKind::DuplicateGroup { .. } => ("review", 0.6),
            _ => ("review", 0.5),
        };

        let reason = simulated_reason(&candidate.kind, age_days);
        let impact = simulated_impact(&candidate.kind);
        RawVerdict {
            id,
            safety: safety.to_string(),
            confidence,
            reason,
            impact,
        }
    }
}

fn simulated_impact(kind: &CandidateKind) -> String {
    match kind {
        CandidateKind::PackageInstaller { .. } => {
            "删除后仅需重新下载安装包，已安装的应用不受影响".to_string()
        }
        CandidateKind::StaleLargeFile => {
            "删除后该文件内容不可恢复，请确认无备份需求".to_string()
        }
        CandidateKind::Archive { .. } => {
            "删除后归档内容不可恢复，已解压的文件不受影响".to_string()
        }
        CandidateKind::DuplicateGroup { .. } => {
            "删除副本后保留的另一份不受影响".to_string()
        }
        _ => "删除影响无法离线确定，建议人工确认".to_string(),
    }
}

fn simulated_reason(kind: &CandidateKind, age_days: u64) -> String {
    match kind {
        CandidateKind::PackageInstaller { .. } => {
            format!("安装包已 {age_days} 天未变动，安装完成后通常可删除")
        }
        CandidateKind::StaleLargeFile => {
            format!("大文件已 {age_days} 天未修改，建议确认是否仍需要")
        }
        CandidateKind::Archive { .. } => "归档内容需要你确认是否仍有保留价值".to_string(),
        CandidateKind::DuplicateGroup { .. } => "存在同名同大小副本，建议保留一份".to_string(),
        _ => "无法离线确定其影响，建议人工确认".to_string(),
    }
}

impl RemoteAdjudicator for SimulatedAdjudicator {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn adjudicate_remote(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<crate::reason::Verdict>, AdjudicateError> {
        // Build the prompt exactly as the real client would, to prove the
        // reusable contract; the simulated answer ignores its text.
        let _prompt = crate::prompt::build_at(batch, "zh", now_ms);
        let raw: Vec<RawVerdict> = batch
            .iter()
            .map(|candidate| Self::raw_for(candidate, now_ms))
            .collect();
        Ok(apply_remote_verdicts(batch, &raw, guardrails, now_ms, &self.provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateKind;
    use sift_core::{ByteSize, NodeKey};
    use std::path::PathBuf;

    fn candidate(kind: CandidateKind, mtime_ms: i64, size: u64) -> Candidate {
        Candidate::new(
            NodeKey::from_bytes(b"c"),
            PathBuf::from("/secret/c"),
            "~/Downloads/c",
            "c",
            false,
            ByteSize::new(size, size),
            mtime_ms,
            kind,
        )
    }

    #[test]
    fn returns_one_verdict_per_candidate_in_order() {
        let now_ms = 1_000_000_000_000;
        let batch = vec![
            candidate(CandidateKind::StaleLargeFile, 0, 1 << 30),
            candidate(
                CandidateKind::PackageInstaller { extension: "dmg".into() },
                0,
                1 << 20,
            ),
        ];
        let verdicts = SimulatedAdjudicator::new()
            .adjudicate_remote(&batch, &Guardrails::default(), now_ms)
            .unwrap();
        assert_eq!(verdicts.len(), 2);
        assert_eq!(verdicts[0].source.label(), "ai:simulated");
    }

    #[test]
    fn old_installer_can_be_safe_but_archive_stays_review() {
        let now_ms = 1_000_000_000_000;
        // 400 days old, >256MB installer
        let old_mtime = now_ms - 400 * 24 * 3600 * 1000;
        let installer = candidate(
            CandidateKind::PackageInstaller { extension: "dmg".into() },
            old_mtime,
            1 << 29,
        );
        let verdicts = SimulatedAdjudicator::new()
            .adjudicate_remote(&[installer], &Guardrails::default(), now_ms)
            .unwrap();
        assert_eq!(verdicts[0].safety, crate::reason::Safety::Safe);

        let archive = candidate(
            CandidateKind::Archive { extension: "zip".into() },
            old_mtime,
            1 << 29,
        );
        let verdicts = SimulatedAdjudicator::new()
            .adjudicate_remote(&[archive], &Guardrails::default(), now_ms)
            .unwrap();
        assert_eq!(verdicts[0].safety, crate::reason::Safety::Review);
    }
}
