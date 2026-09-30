//! Adjudication: turning nominated candidates into safety verdicts.
//!
//! The design constraint that shapes this module: **a language model is never
//! the sole reason something becomes deletable.** A model can confirm what a
//! local rule already found removable, reject it, or raise a concern. It cannot
//! invent a `Safe`, it cannot touch a family whose safety is structural, and it
//! cannot lower a verdict below what the rules concluded.
//!
//! That is enforced by [`sanitize_model_verdict`], which is the only path a
//! remote answer takes into the system. It is deliberately paranoid: unknown
//! ids are dropped, missing answers become [`Safety::fallback`], and a model's
//! `Safe` is downgraded unless the candidate's family allows it *and* the model
//! is confident enough.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::candidate::Candidate;
use crate::reason::{
    clamp_confidence, ConfidencePolicy, PathFingerprint, Reason, Safety, Verdict, VerdictSource,
};

/// Why an adjudication run failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdjudicateError {
    /// The adjudicator is not configured (missing endpoint, key, or model).
    NotConfigured(String),
    /// A transport-level failure: network, TLS, timeout.
    Transport(String),
    /// The service answered, but not with something parseable.
    Response(String),
    /// The caller cancelled.
    Cancelled,
}

impl std::fmt::Display for AdjudicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdjudicateError::NotConfigured(why) => write!(f, "adjudicator not configured: {why}"),
            AdjudicateError::Transport(why) => write!(f, "transport failure: {why}"),
            AdjudicateError::Response(why) => write!(f, "unusable response: {why}"),
            AdjudicateError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for AdjudicateError {}

/// Something that can judge a batch of candidates.
///
/// Implementations are synchronous and may block; callers on a UI thread must
/// run them off it. Implementations must return exactly one verdict per
/// candidate, in the same order — [`adjudicate_all`] enforces this by filling
/// gaps with [`Verdict::unknown`] rather than trusting the implementation.
pub trait Adjudicator: Send + Sync {
    /// Stable name used in logs, verdict sources, and settings.
    fn name(&self) -> &str;

    /// Judge one batch. `now_ms` is passed in so runs are reproducible.
    fn adjudicate_batch(
        &self,
        batch: &[Candidate],
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError>;

    /// Whether this adjudicator transmits anything off the machine. The UI uses
    /// this to require explicit consent before the first run.
    fn is_remote(&self) -> bool {
        false
    }
}

/// The offline adjudicator: local rules only.
///
/// Every candidate already carries the rule that nominated it, so this is a
/// pure re-statement of that rule's conclusion. It exists as an `Adjudicator`
/// so the pipeline has exactly one shape whether or not a model is attached.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleAdjudicator;

impl RuleAdjudicator {
    pub fn new() -> Self {
        Self
    }
}

impl Adjudicator for RuleAdjudicator {
    fn name(&self) -> &str {
        "rules"
    }

    fn adjudicate_batch(
        &self,
        batch: &[Candidate],
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        Ok(batch
            .iter()
            .map(|candidate| rule_verdict(candidate, now_ms))
            .collect())
    }
}

/// The verdict implied by a candidate's own nomination.
///
/// A faithful restatement, not a re-derivation: the rules already decided safety,
/// reason and detail, and reconstructing them here from the rule id would let the
/// two drift — which is exactly how a cache inside an application bundle came
/// back as `Safe` after a rule had already downgraded it.
///
/// A candidate with no nomination is `Review`: visible, but never automatically
/// removable.
pub fn rule_verdict(candidate: &Candidate, now_ms: i64) -> Verdict {
    let fallback_rule = candidate
        .evidence
        .first()
        .map(|evidence| evidence.rule.clone())
        .unwrap_or_else(|| "unknown".to_string());

    match &candidate.nomination {
        Some(nomination) => Verdict::new(
            nomination.safety,
            default_confidence(nomination.safety),
            match &nomination.reason_param {
                Some(param) => Reason::key_with(nomination.reason_key.clone(), vec![param.clone()]),
                None => Reason::key(nomination.reason_key.clone()),
            },
            VerdictSource::rule(nomination.rule.clone()),
            now_ms,
            Some(local_impact(candidate)),
        ),
        None => Verdict::new(
            Safety::fallback(),
            0.0,
            Reason::key("reason.unjudged"),
            VerdictSource::rule(fallback_rule),
            now_ms,
            Some(local_impact(candidate)),
        ),
    }
}

/// The impact of a local-rule verdict, drawn from the cleanup knowledge base so
/// local and model verdicts expose one shape ("what breaks after deletion").
fn local_impact(candidate: &Candidate) -> Reason {
    let plan = crate::cleanup::plan_for(&candidate.kind, &candidate.display_path);
    Reason::key(plan.impact_key)
}

/// Confidence a rule's own conclusion carries. Safe structural patterns are
/// near-certain; anything needing judgement is deliberately middling so it
/// cannot reach an automatic threshold by accident.
fn default_confidence(safety: Safety) -> f32 {
    match safety {
        Safety::Safe => 0.9,
        Safety::Review => 0.5,
        Safety::Keep => 0.0,
    }
}

/// A cached-first adjudicator.
///
/// Reusing a previous judgment is what makes repeated analysis cheap and keeps
/// the product's own conclusions stable: the same cache directory is not
/// re-litigated (or re-transmitted) on every scan.
pub struct CachedAdjudicator<A, C> {
    inner: A,
    cache: C,
    /// Verdicts served from the cache in the last batch, for reporting.
    hits: std::sync::atomic::AtomicUsize,
    misses: std::sync::atomic::AtomicUsize,
}

/// Where [`CachedAdjudicator`] reads and writes verdicts. Implemented by the
/// local store, so this crate stays free of storage detail.
pub trait VerdictCache: Send + Sync {
    fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict>;
    fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict);
}

/// A shared cache is still a cache.
///
/// Both front ends keep the store behind an `Arc` (a monitor thread and the UI
/// need to hold it at once), so the trait follows the pointer rather than
/// forcing every caller to wrap and unwrap.
impl<T: VerdictCache + ?Sized> VerdictCache for std::sync::Arc<T> {
    fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
        (**self).lookup(fingerprint)
    }

    fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict) {
        (**self).store(fingerprint, verdict)
    }
}

impl<A: Adjudicator, C: VerdictCache> CachedAdjudicator<A, C> {
    pub fn new(inner: A, cache: C) -> Self {
        Self {
            inner,
            cache,
            hits: std::sync::atomic::AtomicUsize::new(0),
            misses: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn inner(&self) -> &A {
        &self.inner
    }

    /// Cache hits and misses since construction.
    pub fn stats(&self) -> (usize, usize) {
        (
            self.hits.load(std::sync::atomic::Ordering::Relaxed),
            self.misses.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

impl<A: Adjudicator, C: VerdictCache> Adjudicator for CachedAdjudicator<A, C> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_remote(&self) -> bool {
        self.inner.is_remote()
    }

    fn adjudicate_batch(
        &self,
        batch: &[Candidate],
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        let mut verdicts: Vec<Option<Verdict>> = vec![None; batch.len()];
        let mut pending: Vec<usize> = Vec::new();

        for (index, candidate) in batch.iter().enumerate() {
            match self.cache.lookup(&candidate.fingerprint()) {
                Some(cached) => {
                    self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // Mark the provenance so the UI can say "judged earlier"
                    // without pretending a rule just re-derived it.
                    verdicts[index] = Some(Verdict {
                        source: VerdictSource::Cached,
                        ..cached
                    });
                }
                None => {
                    self.misses
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    pending.push(index);
                }
            }
        }

        if !pending.is_empty() {
            let subset: Vec<Candidate> = pending.iter().map(|&ix| batch[ix].clone()).collect();
            let fresh = self.inner.adjudicate_batch(&subset, now_ms)?;
            for (slot, verdict) in pending.iter().zip(fresh) {
                self.cache.store(&batch[*slot].fingerprint(), &verdict);
                verdicts[*slot] = Some(verdict);
            }
        }

        Ok(verdicts
            .into_iter()
            .map(|verdict| verdict.unwrap_or_else(|| Verdict::unknown(now_ms)))
            .collect())
    }
}

// ---- remote answer handling ------------------------------------------------

/// One verdict as a remote adjudicator reported it, before validation.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawVerdict {
    /// The candidate id the model was given.
    pub id: String,
    /// `safe`, `review`, or `keep`; anything else becomes `Review`.
    pub safety: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub reason: String,
    /// One sentence in the user's language: what breaks after deletion.
    #[serde(default)]
    pub impact: String,
}

/// Parse a model's answer into raw verdicts.
///
/// Accepts either a bare array or an object with a `verdicts` array, because
/// both shapes are common; rejects anything else rather than guessing.
pub fn parse_raw_verdicts(text: &str) -> Result<Vec<RawVerdict>, AdjudicateError> {
    let trimmed = text.trim();
    // Tolerate a fenced code block around the JSON.
    let json = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.trim_start().strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    };

    if let Ok(list) = serde_json::from_str::<Vec<RawVerdict>>(json) {
        return Ok(list);
    }
    #[derive(serde::Deserialize)]
    struct Wrapper {
        verdicts: Vec<RawVerdict>,
    }
    if let Ok(wrapper) = serde_json::from_str::<Wrapper>(json) {
        return Ok(wrapper.verdicts);
    }
    Err(AdjudicateError::Response(
        "expected a JSON array of verdicts".to_string(),
    ))
}

/// How much authority a remote adjudicator is granted.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Guardrails {
    pub confidence: ConfidencePolicy,
    /// Master switch for accepting a model's `Safe` at all. Off means the model
    /// can only ever reject or soften.
    pub allow_model_safe: bool,
}

impl Default for Guardrails {
    fn default() -> Self {
        Self {
            confidence: ConfidencePolicy::default(),
            allow_model_safe: true,
        }
    }
}

/// Why a model's verdict was reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Downgrade {
    /// The family's safety is structural; a model has no say.
    FamilyNotModelAuthorizable,
    /// Model `Safe` is disabled by policy.
    ModelSafeDisabled,
    /// The model was not confident enough to authorize removal.
    LowConfidence,
    /// The answer was unusable.
    Unparsable,
}

impl Downgrade {
    pub const fn reason_key(self) -> &'static str {
        match self {
            Downgrade::FamilyNotModelAuthorizable => "reason.aiNotAuthorized",
            Downgrade::ModelSafeDisabled => "reason.aiConsentRequired",
            Downgrade::LowConfidence => "reason.aiLowConfidence",
            Downgrade::Unparsable => "reason.unjudged",
        }
    }
}

/// The single entry point for a remote answer.
///
/// Returns the verdict the system will actually use, which may be weaker than
/// what the model asked for. This function is where "a model cannot delete your
/// files" is implemented, so it is exhaustively unit-tested.
pub fn sanitize_model_verdict(
    candidate: &Candidate,
    raw: &RawVerdict,
    guardrails: &Guardrails,
    now_ms: i64,
    provider: &str,
) -> Verdict {
    let requested = match raw.safety.trim().to_ascii_lowercase().as_str() {
        "safe" => Safety::Safe,
        "keep" => Safety::Keep,
        // "review" and anything unrecognised land here, which is the safe
        // default.
        _ => Safety::Review,
    };
    let confidence = clamp_confidence(raw.confidence);
    let reason = Reason::sanitized_text(&raw.reason);
    let impact = Reason::sanitized_text(&raw.impact);

    // A model may always be *more* cautious: Keep and Review pass through.
    if requested != Safety::Safe {
        return Verdict::new(
            requested,
            confidence,
            reason.unwrap_or_else(|| Reason::key(requested.label_key())),
            VerdictSource::remote(provider),
            now_ms,
            impact,
        );
    }

    // From here on the model asked for `Safe`, which is the only level that
    // unattended cleanup can act on.
    let downgrade = if !guardrails.allow_model_safe {
        Some(Downgrade::ModelSafeDisabled)
    } else if !candidate.kind.model_may_authorize_safe() {
        Some(Downgrade::FamilyNotModelAuthorizable)
    } else if confidence < guardrails.confidence.remote {
        Some(Downgrade::LowConfidence)
    } else {
        None
    };

    match downgrade {
        None => Verdict::new(
            Safety::Safe,
            confidence,
            reason.unwrap_or_else(|| Reason::key("reason.aiConfirmed")),
            VerdictSource::remote(provider),
            now_ms,
            impact,
        ),
        Some(downgrade) => {
            // Keep the model's explanation, but state plainly that the level
            // was reduced, so the UI never claims more certainty than exists.
            let reason = match reason {
                Some(text) => Reason::key_with(downgrade.reason_key(), vec![text.describe()]),
                None => Reason::key(downgrade.reason_key()),
            };
            Verdict::new(
                Safety::Review,
                confidence,
                reason,
                VerdictSource::remote(provider),
                now_ms,
                impact,
            )
        }
    }
}

/// Match a batch of raw remote verdicts to the candidates they were asked about.
///
/// Ids the model invented are dropped; candidates it skipped receive
/// [`Verdict::unknown`]. Neither can produce a `Safe`.
pub fn apply_remote_verdicts(
    batch: &[Candidate],
    raw: &[RawVerdict],
    guardrails: &Guardrails,
    now_ms: i64,
    provider: &str,
) -> Vec<Verdict> {
    let by_id: HashMap<&str, &RawVerdict> = raw
        .iter()
        .map(|verdict| (verdict.id.as_str(), verdict))
        .collect();

    batch
        .iter()
        .map(|candidate| {
            let id = candidate.key.to_string();
            match by_id.get(id.as_str()) {
                Some(raw) => sanitize_model_verdict(candidate, raw, guardrails, now_ms, provider),
                None => {
                    let mut verdict = Verdict::unknown(now_ms);
                    verdict.reason = Reason::key(Downgrade::Unparsable.reason_key());
                    verdict
                }
            }
        })
        .collect()
}

/// Run `adjudicator` over every candidate in fixed-size batches.
///
/// Any batch failure does not lose the run: the failing batch falls back to
/// [`Verdict::unknown`], so a network problem degrades analysis rather than
/// aborting it — and a degraded answer can never authorize a removal.
pub fn adjudicate_all<A: Adjudicator + ?Sized>(
    adjudicator: &A,
    candidates: &[Candidate],
    batch_size: usize,
    now_ms: i64,
    mut on_batch: impl FnMut(usize, usize),
) -> Vec<Verdict> {
    let batch_size = batch_size.max(1);
    let mut verdicts = Vec::with_capacity(candidates.len());
    let total = candidates.len();
    for (index, chunk) in candidates.chunks(batch_size).enumerate() {
        let offset = index * batch_size;
        on_batch(offset.min(total), total);
        match adjudicator.adjudicate_batch(chunk, now_ms) {
            Ok(batch) if batch.len() == chunk.len() => verdicts.extend(batch),
            Ok(short) => {
                // A short answer is treated as a failure, not padded silently.
                let mut short = short;
                short.resize_with(chunk.len(), || Verdict::unknown(now_ms));
                verdicts.extend(short);
            }
            Err(_) => {
                verdicts.extend(chunk.iter().map(|_| Verdict::unknown(now_ms)));
            }
        }
    }
    verdicts
}

// ---- the result of a run ---------------------------------------------------

/// A candidate paired with the verdict that will be shown for it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AnalyzedItem {
    pub candidate: Candidate,
    pub verdict: Verdict,
}

impl AnalyzedItem {
    pub fn new(candidate: Candidate, verdict: Verdict) -> Self {
        Self { candidate, verdict }
    }

    /// Whether this is one of the conclusions the product considers
    /// definitively cleanable.
    pub fn is_known_cleanable(&self) -> bool {
        self.verdict.safety == Safety::Safe
    }
}

/// Everything one analysis run concluded.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AnalysisReport {
    pub items: Vec<AnalyzedItem>,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    /// How many items each verdict source contributed, for the UI's
    /// "N by rules / M by AI" line.
    pub source_counts: BTreeMap<String, usize>,
    /// Candidates whose batch failed and were left unjudged.
    pub unjudged: usize,
}

impl AnalysisReport {
    pub fn new(items: Vec<AnalyzedItem>, started_at_ms: i64, finished_at_ms: i64) -> Self {
        let mut source_counts = BTreeMap::new();
        let mut unjudged = 0;
        for item in &items {
            *source_counts
                .entry(item.verdict.source.label())
                .or_insert(0) += 1;
            if item.verdict.source.label() == "rule:fallback" {
                unjudged += 1;
            }
        }
        Self {
            items,
            started_at_ms,
            finished_at_ms,
            source_counts,
            unjudged,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Bytes reclaimable if every `Safe` and `Review` item were removed.
    pub fn reclaimable(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.verdict.safety.is_removable())
            .map(|item| item.candidate.reclaimable())
            .sum()
    }

    /// Bytes reclaimable with no user decision — the number the monitor may act
    /// on unattended.
    pub fn safe_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.verdict.safety.is_automatic())
            .map(|item| item.candidate.reclaimable())
            .sum()
    }

    /// The definitively cleanable list: what gets persisted as the basis for
    /// later analysis and automatic cleanup.
    pub fn known_cleanable(&self) -> Vec<&AnalyzedItem> {
        self.items
            .iter()
            .filter(|item| item.is_known_cleanable())
            .collect()
    }

    /// Items sorted the way the product lists them: reclaimable bytes desc.
    pub fn sorted_by_size(&self) -> Vec<&AnalyzedItem> {
        let mut items: Vec<&AnalyzedItem> = self.items.iter().collect();
        items.sort_by(|left, right| {
            right
                .candidate
                .reclaimable()
                .cmp(&left.candidate.reclaimable())
                .then_with(|| left.candidate.key.cmp(&right.candidate.key))
        });
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{CandidateKind, Evidence};
    use sift_core::{ByteSize, NodeKey};
    use std::path::PathBuf;

    /// The conclusion a rule with this id reaches, for fixtures. Kept in the
    /// test so a production change to a rule shows up as a failing test rather
    /// than being silently mirrored here.
    fn nominated_safety(rule: &str) -> Safety {
        match rule {
            "dir.trash" | "dir.node_modules" | "file.os_noise" => Safety::Safe,
            _ => Safety::Review,
        }
    }

    fn candidate(kind: CandidateKind, rule: &str) -> Candidate {
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(rule.as_bytes()),
            PathBuf::from("/Users/me/x"),
            "~/x",
            "x",
            matches!(
                kind,
                CandidateKind::RebuildableCache { .. } | CandidateKind::Trash
            ),
            ByteSize::new(1 << 30, 1 << 30),
            0,
            kind,
        );
        candidate
            .evidence
            .push(Evidence::new(rule, "detail.patternMatch"));
        candidate.with_nomination(crate::candidate::Nomination {
            rule: rule.to_string(),
            safety: nominated_safety(rule),
            reason_key: reason_key_for(rule).to_string(),
            // A cache reason names the toolchain, which is what makes the reason
            // sentence useful to a reader.
            reason_param: match rule {
                "dir.node_modules" => Some("npm".to_string()),
                _ => None,
            },
            detail_key: "detail.patternMatch".to_string(),
        })
    }

    fn reason_key_for(rule: &str) -> &'static str {
        match rule {
            "dir.trash" => "reason.trash",
            "dir.node_modules" => "reason.rebuildableCache",
            "file.stale_large" => "reason.staleLargeFile",
            "file.archive" => "reason.archive",
            _ => "reason.unknown",
        }
    }

    fn raw(id: &str, safety: &str, confidence: f32, reason: &str) -> RawVerdict {
        RawVerdict {
            id: id.to_string(),
            safety: safety.to_string(),
            confidence,
            reason: reason.to_string(),
            impact: String::new(),
        }
    }

    #[test]
    fn model_cannot_mark_a_structural_family_safe() {
        let trash = candidate(CandidateKind::Trash, "dir.trash");
        let verdict = sanitize_model_verdict(
            &trash,
            &raw(&trash.key.to_string(), "safe", 0.99, "looks fine"),
            &Guardrails::default(),
            0,
            "remote",
        );
        assert_eq!(verdict.safety, Safety::Review, "trash safety is structural");
        assert!(!verdict.is_automatically_removable(&ConfidencePolicy::default()));
    }

    #[test]
    fn model_safe_is_downgraded_below_the_confidence_floor() {
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let verdict = sanitize_model_verdict(
            &stale,
            &raw(&stale.key.to_string(), "safe", 0.5, "probably unused"),
            &Guardrails::default(),
            0,
            "remote",
        );
        assert_eq!(verdict.safety, Safety::Review);
        match verdict.reason {
            Reason::Key { key, .. } => assert_eq!(key, "reason.aiLowConfidence"),
            other => panic!("expected a key, got {other:?}"),
        }
    }

    #[test]
    fn confident_model_safe_is_accepted_for_an_authorizable_family() {
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let verdict = sanitize_model_verdict(
            &stale,
            &raw(&stale.key.to_string(), "safe", 0.95, "an old installer"),
            &Guardrails::default(),
            0,
            "remote",
        );
        assert_eq!(verdict.safety, Safety::Safe);
        assert!(verdict.is_automatically_removable(&ConfidencePolicy::default()));
        assert!(verdict.source.is_model());
    }

    #[test]
    fn model_can_always_be_more_cautious() {
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        for (safety, expected) in [("keep", Safety::Keep), ("review", Safety::Review)] {
            let verdict = sanitize_model_verdict(
                &stale,
                &raw(&stale.key.to_string(), safety, 0.1, "risky"),
                &Guardrails::default(),
                0,
                "remote",
            );
            assert_eq!(verdict.safety, expected, "{safety} must pass through");
        }
    }

    #[test]
    fn model_safe_can_be_globally_disabled() {
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let guardrails = Guardrails {
            allow_model_safe: false,
            ..Guardrails::default()
        };
        let verdict = sanitize_model_verdict(
            &stale,
            &raw(&stale.key.to_string(), "safe", 1.0, "certain"),
            &guardrails,
            0,
            "remote",
        );
        assert_eq!(verdict.safety, Safety::Review);
    }

    #[test]
    fn unknown_safety_strings_become_review() {
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        for weird in ["", "SAFE!", "definitely-fine", "delete"] {
            let verdict = sanitize_model_verdict(
                &stale,
                &raw(&stale.key.to_string(), weird, 1.0, "x"),
                &Guardrails::default(),
                0,
                "remote",
            );
            assert_eq!(verdict.safety, Safety::Review, "{weird:?}");
        }
    }

    #[test]
    fn missing_answers_and_invented_ids_are_handled() {
        let a = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let mut b = candidate(
            CandidateKind::Archive {
                extension: "zip".into(),
            },
            "file.archive",
        );
        b.key = NodeKey::from_bytes(b"second");
        let batch = vec![a.clone(), b];

        let raw_answers = vec![
            raw(&a.key.to_string(), "safe", 0.99, "old installer"),
            // An id that was never asked about must not create anything.
            raw(
                "n-00000000000000000000000000000000",
                "safe",
                1.0,
                "invented",
            ),
        ];
        let verdicts = apply_remote_verdicts(&batch, &raw_answers, &Guardrails::default(), 0, "remote");
        assert_eq!(verdicts.len(), 2);
        assert_eq!(verdicts[0].safety, Safety::Safe);
        // The skipped candidate is Review, never Safe.
        assert_eq!(verdicts[1].safety, Safety::Review);
    }

    #[test]
    fn parsing_accepts_both_json_shapes_and_fences() {
        let array = r#"[{"id":"n-1","safety":"review","confidence":0.4,"reason":"maybe"}]"#;
        assert_eq!(parse_raw_verdicts(array).unwrap().len(), 1);
        let wrapped = r#"{"verdicts":[{"id":"n-1","safety":"keep","reason":"no"}]}"#;
        assert_eq!(parse_raw_verdicts(wrapped).unwrap().len(), 1);
        let fenced = format!("```json\n{array}\n```");
        assert_eq!(parse_raw_verdicts(&fenced).unwrap().len(), 1);
        assert!(parse_raw_verdicts("not json at all").is_err());
    }

    #[test]
    fn missing_confidence_field_defaults_to_zero() {
        let json = r#"[{"id":"n-1","safety":"safe","reason":"x"}]"#;
        let parsed = parse_raw_verdicts(json).unwrap();
        assert_eq!(parsed[0].confidence, 0.0);
        let candidate = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let verdict = sanitize_model_verdict(&candidate, &parsed[0], &Guardrails::default(), 0, "remote");
        assert_eq!(verdict.safety, Safety::Review, "no confidence, no safe");
    }

    #[test]
    fn rule_adjudicator_restates_the_rule() {
        let node_modules = candidate(
            CandidateKind::RebuildableCache { tool: "npm".into() },
            "dir.node_modules",
        );
        let verdict = rule_verdict(&node_modules, 7);
        assert_eq!(verdict.safety, Safety::Safe);
        assert_eq!(verdict.source, VerdictSource::rule("dir.node_modules"));
        assert_eq!(verdict.judged_at_ms, 7);
        match verdict.reason {
            Reason::Key { key, params } => {
                assert_eq!(key, "reason.rebuildableCache");
                assert_eq!(params.len(), 1);
            }
            other => panic!("expected a key, got {other:?}"),
        }
    }

    #[test]
    fn a_candidate_with_no_evidence_is_never_safe() {
        let unknown = Candidate::new(
            NodeKey::from_bytes(b"n"),
            PathBuf::from("/x/y"),
            "~/y",
            "y",
            false,
            ByteSize::new(1, 1),
            0,
            CandidateKind::UserMarked,
        );
        let verdict = rule_verdict(&unknown, 0);
        assert_eq!(verdict.safety, Safety::Review);
        assert_eq!(verdict.confidence, 0.0);
    }

    #[test]
    fn adjudicate_all_fills_failures_without_authorizing_removal() {
        struct Failing;
        impl Adjudicator for Failing {
            fn name(&self) -> &str {
                "failing"
            }
            fn adjudicate_batch(
                &self,
                _batch: &[Candidate],
                _now_ms: i64,
            ) -> Result<Vec<Verdict>, AdjudicateError> {
                Err(AdjudicateError::Transport("offline".into()))
            }
        }
        let candidates = vec![
            candidate(CandidateKind::Trash, "dir.trash"),
            candidate(CandidateKind::StaleLargeFile, "file.stale_large"),
            candidate(
                CandidateKind::Archive {
                    extension: "zip".into(),
                },
                "file.archive",
            ),
        ];
        let verdicts = adjudicate_all(&Failing, &candidates, 2, 0, |_, _| {});
        assert_eq!(verdicts.len(), 3);
        assert!(verdicts
            .iter()
            .all(|verdict| verdict.safety == Safety::Review));
        assert!(verdicts
            .iter()
            .all(|verdict| !verdict.is_automatically_removable(&ConfidencePolicy::default())));
    }

    #[test]
    fn an_arc_wrapped_cache_still_works() {
        use std::sync::Mutex as StdMutex;

        #[derive(Default)]
        struct Shared(StdMutex<Vec<(PathFingerprint, Verdict)>>);
        impl VerdictCache for Shared {
            fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
                self.0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(stored, _)| stored.still_describes(fingerprint))
                    .map(|(_, verdict)| verdict.clone())
            }
            fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict) {
                self.0.lock().unwrap().push((*fingerprint, verdict.clone()));
            }
        }

        let shared = std::sync::Arc::new(Shared::default());
        let adjudicator =
            CachedAdjudicator::new(RuleAdjudicator::new(), std::sync::Arc::clone(&shared));
        let candidates = vec![candidate(CandidateKind::Trash, "dir.trash")];
        let first = adjudicator.adjudicate_batch(&candidates, 0).unwrap();
        assert_eq!(first[0].source, VerdictSource::rule("dir.trash"));
        let second = adjudicator.adjudicate_batch(&candidates, 1).unwrap();
        assert_eq!(second[0].source, VerdictSource::Cached);
        assert_eq!(shared.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn cached_adjudicator_reuses_and_records() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct MemCache(Mutex<Vec<(PathFingerprint, Verdict)>>);
        impl VerdictCache for MemCache {
            fn lookup(&self, fingerprint: &PathFingerprint) -> Option<Verdict> {
                self.0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(stored, _)| stored.still_describes(fingerprint))
                    .map(|(_, verdict)| verdict.clone())
            }
            fn store(&self, fingerprint: &PathFingerprint, verdict: &Verdict) {
                self.0.lock().unwrap().push((*fingerprint, verdict.clone()));
            }
        }

        let cache = MemCache::default();
        let adjudicator = CachedAdjudicator::new(RuleAdjudicator::new(), cache);
        let candidates = vec![candidate(CandidateKind::Trash, "dir.trash")];

        let first = adjudicator.adjudicate_batch(&candidates, 0).unwrap();
        assert_eq!(first[0].safety, Safety::Safe);
        assert_eq!(first[0].source, VerdictSource::rule("dir.trash"));
        assert_eq!(adjudicator.stats(), (0, 1));

        let second = adjudicator.adjudicate_batch(&candidates, 1).unwrap();
        assert_eq!(second[0].safety, Safety::Safe);
        assert_eq!(second[0].source, VerdictSource::Cached);
        assert_eq!(adjudicator.stats(), (1, 1));
    }

    #[test]
    fn report_summarises_safe_and_total() {
        let trash = candidate(CandidateKind::Trash, "dir.trash");
        let stale = candidate(CandidateKind::StaleLargeFile, "file.stale_large");
        let items = vec![
            AnalyzedItem::new(trash.clone(), rule_verdict(&trash, 0)),
            AnalyzedItem::new(stale.clone(), rule_verdict(&stale, 0)),
        ];
        let report = AnalysisReport::new(items, 0, 10);
        assert_eq!(report.items.len(), 2);
        // Both are removable, only the trash is unattended-removable.
        assert_eq!(report.reclaimable(), (1 << 30) * 2);
        assert_eq!(report.safe_bytes(), 1 << 30);
        assert_eq!(report.known_cleanable().len(), 1);
        assert!(report.source_counts.contains_key("rule:dir.trash"));
    }
}
