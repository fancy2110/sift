//! What the user decided, and what that implies.
//!
//! Two jobs:
//!
//! * **Memory.** Every removal or deliberate keep is recorded by fingerprint, so
//!   the product can answer "you have removed this three times" instead of
//!   asking the same question forever.
//! * **Habit mining.** A decision that keeps coming back is a routine the user
//!   has been performing by hand. Suggesting it is the difference between a
//!   cleanup tool and a habit the machine learns.
//!
//! The log is deliberately weak on its own: it is a list of observations, not a
//! policy. Turning it into unattended action is [`crate::adjudicate`]'s
//! [`crate::VerdictSource::Learned`] path, which still has to clear the
//! confidence and safety bars.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::adjudicate::{AdjudicateError, Adjudicator};
use crate::reason::{PathFingerprint, Reason, Safety, Verdict, VerdictSource};

/// What the user did with a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionOutcome {
    /// Sent to the trash.
    Removed,
    /// Explicitly kept — as informative as a removal, and the reason a
    /// suggestion is never one-sided.
    Kept,
    /// Left alone without an explicit choice. Recorded but never used to
    /// suggest anything: silence is not a decision.
    Ignored,
}

impl DecisionOutcome {
    pub const fn is_explicit(self) -> bool {
        matches!(self, DecisionOutcome::Removed | DecisionOutcome::Kept)
    }
}

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Decision {
    pub fingerprint: PathFingerprint,
    /// Redacted path, for grouping and display only.
    pub display_path: String,
    pub name: String,
    /// The candidate family token, e.g. `rebuildableCache`.
    pub kind_token: String,
    /// Bytes the item occupied when decided.
    pub size: u64,
    pub outcome: DecisionOutcome,
    pub decided_at_ms: i64,
}

impl Decision {
    pub fn new(
        fingerprint: PathFingerprint,
        display_path: impl Into<String>,
        name: impl Into<String>,
        kind_token: impl Into<String>,
        size: u64,
        outcome: DecisionOutcome,
        decided_at_ms: i64,
    ) -> Self {
        Self {
            fingerprint,
            display_path: display_path.into(),
            name: name.into(),
            kind_token: kind_token.into(),
            size,
            outcome,
            decided_at_ms,
        }
    }

    /// The local day this decision fell on, used to require that a habit
    /// repeats over time rather than in one enthusiastic afternoon.
    pub fn day(&self) -> i64 {
        self.decided_at_ms.div_euclid(86_400_000)
    }
}

/// An append-only list of decisions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DecisionLog {
    pub entries: Vec<Decision>,
}

impl DecisionLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Append a decision, keeping the log newest-last.
    pub fn record(&mut self, decision: Decision) {
        self.entries.push(decision);
    }

    /// How many times this exact fingerprint was removed.
    pub fn times_removed(&self, fingerprint: &PathFingerprint) -> usize {
        self.entries
            .iter()
            .filter(|entry| {
                entry.outcome == DecisionOutcome::Removed
                    && entry.fingerprint.still_describes(fingerprint)
            })
            .count()
    }

    /// How many times this exact fingerprint was kept.
    pub fn times_kept(&self, fingerprint: &PathFingerprint) -> usize {
        self.entries
            .iter()
            .filter(|entry| {
                entry.outcome == DecisionOutcome::Kept
                    && entry.fingerprint.still_describes(fingerprint)
            })
            .count()
    }

    /// Whether the user has kept this *path* often enough that the product
    /// should stop suggesting it, regardless of size changes.
    pub fn is_habitually_kept(&self, key: sift_core::NodeKey, threshold: usize) -> bool {
        let kept = self
            .entries
            .iter()
            .filter(|entry| entry.fingerprint.key == key && entry.outcome == DecisionOutcome::Kept)
            .count();
        let removed = self
            .entries
            .iter()
            .filter(|entry| {
                entry.fingerprint.key == key && entry.outcome == DecisionOutcome::Removed
            })
            .count();
        kept >= threshold && kept > removed
    }

    /// Whether the user has removed whatever sits at this *path* often enough
    /// that the product should treat a fresh occurrence as cleanable, regardless
    /// of the new instance's size or mtime (a rebuilt cache is the same cleanup
    /// question).
    pub fn is_habitually_removed(&self, key: sift_core::NodeKey, threshold: usize) -> bool {
        let removed = self
            .entries
            .iter()
            .filter(|entry| {
                entry.fingerprint.key == key && entry.outcome == DecisionOutcome::Removed
            })
            .count();
        let kept = self
            .entries
            .iter()
            .filter(|entry| entry.fingerprint.key == key
                && entry.outcome == DecisionOutcome::Kept)
            .count();
        removed >= threshold && removed > kept
    }

    /// Group explicit decisions by the thing the user actually recognized:
    /// the name plus the family. Two `node_modules` in different projects are
    /// the same habit; the same name in a different family is not.
    fn groups(&self) -> HashMap<(String, String), GroupStats> {
        let mut groups: HashMap<(String, String), GroupStats> = HashMap::new();
        for entry in &self.entries {
            if !entry.outcome.is_explicit() {
                continue;
            }
            let stats = groups
                .entry((entry.name.clone(), entry.kind_token.clone()))
                .or_default();
            stats.observe(entry);
        }
        groups
    }

    /// Suggest routines from repeated decisions.
    ///
    /// A suggestion requires `min_occurrences` explicit decisions spread over at
    /// least `min_days` distinct days, so a single cleanup session cannot invent
    /// a recurring task. A group that was removed more often than kept is
    /// suggested for removal; anything the user keeps more often is surfaced as
    /// a "keep" suggestion, which suppresses future prompts.
    pub fn suggest_routines(
        &self,
        min_occurrences: usize,
        min_days: usize,
    ) -> Vec<RoutineSuggestion> {
        let mut suggestions: Vec<RoutineSuggestion> = self
            .groups()
            .into_iter()
            .filter_map(|((name, kind_token), stats)| {
                if stats.occurrences() < min_occurrences || stats.distinct_days() < min_days {
                    return None;
                }
                let reason = if stats.removed > stats.kept {
                    SuggestionReason::RepeatedRemoval
                } else if stats.kept > stats.removed {
                    SuggestionReason::RepeatedKept
                } else {
                    return None; // an even split is not a habit
                };
                Some(RoutineSuggestion {
                    name,
                    kind_token,
                    occurrences: stats.occurrences(),
                    distinct_days: stats.distinct_days(),
                    average_bytes: stats.average_bytes(),
                    total_bytes: stats.total_bytes,
                    reason,
                    cadence: stats.suggested_cadence(),
                })
            })
            .collect();

        // Biggest wins first; a stable tie-break keeps the order reproducible.
        suggestions.sort_by(|left, right| {
            right
                .total_bytes
                .cmp(&left.total_bytes)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.kind_token.cmp(&right.kind_token))
        });
        suggestions
    }
}

// ---- learned verdicts ------------------------------------------------------

/// Confidence a verdict inferred from the user's own decisions carries.
///
/// Kept equal to [`crate::reason::ConfidencePolicy::default`]'s `learned`
/// threshold: a learned verdict is usable unattended exactly when the default
/// policy says a learned source may act.
pub const LEARNED_CONFIDENCE: f32 = 0.7;

/// When the user's own decision history may revise a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HabitPolicy {
    /// Keep-decisions at one path key that suppress every suggestion there: a
    /// `Safe` or `Review` verdict becomes `Keep`.
    pub keep_threshold: usize,
    /// Remove-decisions at one path key that let a `Review` verdict become
    /// `Safe`. Structural `Keep` verdicts are never overridden.
    pub remove_threshold: usize,
}

impl Default for HabitPolicy {
    fn default() -> Self {
        Self {
            keep_threshold: 3,
            remove_threshold: 3,
        }
    }
}

impl HabitPolicy {
    pub fn with_keep_threshold(mut self, threshold: usize) -> Self {
        self.keep_threshold = threshold;
        self
    }

    pub fn with_remove_threshold(mut self, threshold: usize) -> Self {
        self.remove_threshold = threshold;
        self
    }
}

/// An adjudicator that lets the user's own decision history revise verdicts.
///
/// This is the only constructor of [`VerdictSource::Learned`], and with it the
/// only path where [`crate::reason::ConfidencePolicy::learned`] is reachable.
/// Two revisions, each one-sided in the cautious direction:
///
/// * A path the user has kept past the threshold is forced to `Keep`: the
///   product stops asking about it.
/// * A `Review` verdict for a path the user has removed past the threshold is
///   promoted to `Safe`: the product learned the answer. The entry still enters
///   the cleanable list with `approved_for_auto = false`, so a learned `Safe`
///   cannot be deleted unattended until the user separately approves it.
///
/// A structural `Keep` is never revised upward, and revisions require the
/// relevant decisions to outnumber the opposite kind, so a mixed history does
/// nothing.
pub struct HabitAdjudicator<A> {
    inner: A,
    habits: DecisionLog,
    policy: HabitPolicy,
}

impl<A> HabitAdjudicator<A> {
    pub fn new(inner: A, habits: DecisionLog, policy: HabitPolicy) -> Self {
        Self {
            inner,
            habits,
            policy,
        }
    }

    pub fn inner(&self) -> &A {
        &self.inner
    }

    pub fn habits(&self) -> &DecisionLog {
        &self.habits
    }

    pub fn policy(&self) -> &HabitPolicy {
        &self.policy
    }
}

impl<A: Adjudicator> Adjudicator for HabitAdjudicator<A> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_remote(&self) -> bool {
        self.inner.is_remote()
    }

    fn adjudicate_batch(
        &self,
        batch: &[crate::Candidate],
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        let mut verdicts = self.inner.adjudicate_batch(batch, now_ms)?;
        for (candidate, verdict) in batch.iter().zip(verdicts.iter_mut()) {
            let key = candidate.key;
            if verdict.safety != Safety::Keep
                && self.habits.is_habitually_kept(key, self.policy.keep_threshold)
            {
                let impact = verdict.impact.clone();
                *verdict = Verdict::new(
                    Safety::Keep,
                    LEARNED_CONFIDENCE,
                    Reason::key("reason.habituallyKept"),
                    VerdictSource::Learned,
                    now_ms,
                    impact,
                );
            } else if verdict.safety == Safety::Review
                && self.habits.is_habitually_removed(key, self.policy.remove_threshold)
            {
                let impact = verdict.impact.clone();
                *verdict = Verdict::new(
                    Safety::Safe,
                    LEARNED_CONFIDENCE,
                    Reason::key("reason.habituallyRemoved"),
                    VerdictSource::Learned,
                    now_ms,
                    impact,
                );
            }
        }
        Ok(verdicts)
    }
}

#[derive(Debug, Default)]
struct GroupStats {
    removed: usize,
    kept: usize,
    total_bytes: u64,
    days: std::collections::HashSet<i64>,
}

impl GroupStats {
    fn observe(&mut self, entry: &Decision) {
        match entry.outcome {
            DecisionOutcome::Removed => self.removed += 1,
            DecisionOutcome::Kept => self.kept += 1,
            DecisionOutcome::Ignored => return,
        }
        self.total_bytes = self.total_bytes.saturating_add(entry.size);
        self.days.insert(entry.day());
    }

    fn occurrences(&self) -> usize {
        self.removed + self.kept
    }

    fn distinct_days(&self) -> usize {
        self.days.len()
    }

    fn average_bytes(&self) -> u64 {
        if self.occurrences() == 0 {
            0
        } else {
            self.total_bytes / self.occurrences() as u64
        }
    }

    /// More distinct days means a more entrenched habit.
    fn suggested_cadence(&self) -> Cadence {
        if self.days.len() >= 8 {
            Cadence::Weekly
        } else {
            Cadence::Monthly
        }
    }
}

/// Why a routine is being suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionReason {
    /// The user has removed this kind of thing repeatedly.
    RepeatedRemoval,
    /// The user has kept this kind of thing repeatedly: stop asking.
    RepeatedKept,
}

impl SuggestionReason {
    pub const fn label_key(self) -> &'static str {
        match self {
            SuggestionReason::RepeatedRemoval => "routine.reason.repeatedRemoval",
            SuggestionReason::RepeatedKept => "routine.reason.repeatedKept",
        }
    }
}

/// How often a suggested routine should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cadence {
    Weekly,
    Monthly,
}

impl Cadence {
    pub const fn label_key(self) -> &'static str {
        match self {
            Cadence::Weekly => "routine.cadence.weekly",
            Cadence::Monthly => "routine.cadence.monthly",
        }
    }
}

/// A routine the user's own behaviour suggests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RoutineSuggestion {
    pub name: String,
    pub kind_token: String,
    pub occurrences: usize,
    pub distinct_days: usize,
    pub average_bytes: u64,
    pub total_bytes: u64,
    pub reason: SuggestionReason,
    pub cadence: Cadence,
}

impl RoutineSuggestion {
    /// Whether accepting this suggestion would delete anything. A "keep"
    /// suggestion only suppresses prompts.
    pub fn is_destructive(&self) -> bool {
        self.reason == SuggestionReason::RepeatedRemoval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateKind;
    use crate::reason::ConfidencePolicy;
    use sift_core::{ByteSize, NodeKey};
    use std::path::PathBuf;

    const DAY: i64 = 86_400_000;

    fn decision(name: &str, kind: &str, outcome: DecisionOutcome, day: i64, size: u64) -> Decision {
        Decision::new(
            PathFingerprint::new(NodeKey::from_bytes(name.as_bytes()), size, 0, true),
            format!("~/projects/{name}"),
            name,
            kind,
            size,
            outcome,
            day * DAY,
        )
    }

    #[test]
    fn counts_removals_and_keeps_by_fingerprint() {
        let fingerprint = PathFingerprint::new(NodeKey::from_bytes(b"node_modules"), 100, 0, true);
        let mut log = DecisionLog::new();
        log.record(decision(
            "node_modules",
            "rebuildableCache",
            DecisionOutcome::Removed,
            0,
            100,
        ));
        log.record(decision(
            "node_modules",
            "rebuildableCache",
            DecisionOutcome::Removed,
            1,
            100,
        ));
        log.record(decision(
            "target",
            "rebuildableCache",
            DecisionOutcome::Kept,
            1,
            200,
        ));

        assert_eq!(log.times_removed(&fingerprint), 2);
        assert_eq!(log.times_kept(&fingerprint), 0);
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn ignored_entries_are_not_decisions() {
        let mut log = DecisionLog::new();
        for day in 0..5 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Ignored,
                day,
                100,
            ));
        }
        assert!(
            log.suggest_routines(3, 3).is_empty(),
            "silence must not become a routine"
        );
    }

    #[test]
    fn repeated_removal_becomes_a_suggestion() {
        let mut log = DecisionLog::new();
        for day in 0..4 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                500,
            ));
        }
        let suggestions = log.suggest_routines(3, 3);
        assert_eq!(suggestions.len(), 1);
        let suggestion = &suggestions[0];
        assert_eq!(suggestion.name, "node_modules");
        assert_eq!(suggestion.occurrences, 4);
        assert_eq!(suggestion.distinct_days, 4);
        assert_eq!(suggestion.average_bytes, 500);
        assert_eq!(suggestion.reason, SuggestionReason::RepeatedRemoval);
        assert!(suggestion.is_destructive());
    }

    #[test]
    fn repeated_keeps_suppress_rather_than_delete() {
        let mut log = DecisionLog::new();
        for day in 0..4 {
            log.record(decision(
                "Caches",
                "cacheDirectory",
                DecisionOutcome::Kept,
                day,
                100,
            ));
        }
        let suggestions = log.suggest_routines(3, 3);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].reason, SuggestionReason::RepeatedKept);
        assert!(!suggestions[0].is_destructive());
    }

    #[test]
    fn one_session_cannot_invent_a_habit() {
        let mut log = DecisionLog::new();
        // Ten removals, all on the same day.
        for _ in 0..10 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                5,
                100,
            ));
        }
        assert!(
            log.suggest_routines(3, 3).is_empty(),
            "a burst on one day is not a routine"
        );
        // The same ten across ten days is.
        let mut spread = DecisionLog::new();
        for day in 0..10 {
            spread.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
        }
        assert_eq!(spread.suggest_routines(3, 3).len(), 1);
    }

    #[test]
    fn an_even_split_is_not_a_habit() {
        let mut log = DecisionLog::new();
        for day in 0..2 {
            log.record(decision(
                "target",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
            log.record(decision(
                "target",
                "rebuildableCache",
                DecisionOutcome::Kept,
                day + 10,
                100,
            ));
        }
        assert!(log.suggest_routines(3, 3).is_empty());
    }

    #[test]
    fn different_families_do_not_merge() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
            log.record(decision(
                "node_modules",
                "userMarked",
                DecisionOutcome::Kept,
                day,
                100,
            ));
        }
        let suggestions = log.suggest_routines(3, 2);
        // Each family is its own habit: one removals, one keeps.
        assert_eq!(suggestions.len(), 2);
        let removal = suggestions
            .iter()
            .find(|s| s.kind_token == "rebuildableCache")
            .unwrap();
        assert_eq!(removal.reason, SuggestionReason::RepeatedRemoval);
        let kept = suggestions
            .iter()
            .find(|s| s.kind_token == "userMarked")
            .unwrap();
        assert_eq!(kept.reason, SuggestionReason::RepeatedKept);
    }

    #[test]
    fn suggestions_are_ordered_by_impact() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "small",
                "archive",
                DecisionOutcome::Removed,
                day,
                10,
            ));
            log.record(decision(
                "large",
                "archive",
                DecisionOutcome::Removed,
                day,
                10_000,
            ));
        }
        let suggestions = log.suggest_routines(3, 2);
        assert_eq!(suggestions[0].name, "large");
        assert_eq!(suggestions[1].name, "small");
    }

    #[test]
    fn habitual_keeps_are_recognizable_by_path() {
        let key = NodeKey::from_bytes(b"/Users/me/special");
        let mut log = DecisionLog::new();
        for day in 0..4 {
            log.record(Decision::new(
                PathFingerprint::new(key, 100 + day as u64, 0, true),
                "~/special",
                "special",
                "userMarked",
                100,
                DecisionOutcome::Kept,
                day * DAY,
            ));
        }
        assert!(log.is_habitually_kept(key, 3));
        // A path that was removed more often than kept is not protected.
        let other = NodeKey::from_bytes(b"/Users/me/other");
        let mut mixed = DecisionLog::new();
        for day in 0..3 {
            mixed.record(Decision::new(
                PathFingerprint::new(other, 1, 0, true),
                "~/other",
                "other",
                "x",
                1,
                DecisionOutcome::Removed,
                day * DAY,
            ));
            mixed.record(Decision::new(
                PathFingerprint::new(other, 2, 0, true),
                "~/other",
                "other",
                "x",
                1,
                DecisionOutcome::Kept,
                (day + 5) * DAY,
            ));
        }
        assert!(!mixed.is_habitually_kept(other, 2));
    }

    #[test]
    fn log_round_trips_through_json() {
        let mut log = DecisionLog::new();
        log.record(decision(
            "node_modules",
            "rebuildableCache",
            DecisionOutcome::Removed,
            3,
            42,
        ));
        let json = serde_json::to_string(&log).unwrap();
        let back: DecisionLog = serde_json::from_str(&json).unwrap();
        assert_eq!(log, back);
    }

    #[test]
    fn habitual_removal_is_recognizable_by_key_alone() {
        let key = NodeKey::from_bytes(b"/Users/me/repeat");
        let mut log = DecisionLog::new();
        // Each instance differs in size and mtime: a rebuilt cache is the same
        // cleanup question, matched by key.
        for day in 0..4 {
            log.record(Decision::new(
                PathFingerprint::new(key, 100 + day as u64, day, true),
                "~/repeat",
                "repeat",
                "x",
                100,
                DecisionOutcome::Removed,
                day * DAY,
            ));
        }
        assert!(log.is_habitually_removed(key, 3));
        assert!(!log.is_habitually_kept(key, 3));
    }

    /// An inner adjudicator that gives every candidate one fixed verdict.
    struct Fixed(Safety, f32);
    impl Adjudicator for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn adjudicate_batch(
            &self,
            batch: &[crate::Candidate],
            now_ms: i64,
        ) -> Result<Vec<Verdict>, AdjudicateError> {
            Ok(batch
                .iter()
                .map(|_| {
                    Verdict::new(
                        self.0,
                        self.1,
                        Reason::key("reason.x"),
                        VerdictSource::rule("fixed"),
                        now_ms,
                        None,
                    )
                })
                .collect())
        }
    }

    fn candidate_named(name: &str) -> crate::Candidate {
        crate::Candidate::new(
            NodeKey::from_bytes(name.as_bytes()),
            PathBuf::from(format!("/Users/me/{name}")),
            format!("~/{name}"),
            name,
            false,
            ByteSize::new(100, 100),
            0,
            CandidateKind::UserMarked,
        )
    }

    #[test]
    fn habit_adjudicator_forces_keep_even_over_a_safe() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "special",
                "userMarked",
                DecisionOutcome::Kept,
                day,
                100,
            ));
        }
        let adjudicator =
            HabitAdjudicator::new(Fixed(Safety::Safe, 0.9), log, HabitPolicy::default());
        let batch = vec![candidate_named("special")];
        let verdicts = adjudicator.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Keep);
        assert_eq!(verdicts[0].source, VerdictSource::Learned);
        assert_eq!(verdicts[0].confidence, LEARNED_CONFIDENCE);
        match &verdicts[0].reason {
            Reason::Key { key, .. } => assert_eq!(key, "reason.habituallyKept"),
            other => panic!("expected a key, got {other:?}"),
        }
    }

    #[test]
    fn habit_adjudicator_promotes_review_to_safe_after_repeated_removal() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
        }
        let adjudicator =
            HabitAdjudicator::new(Fixed(Safety::Review, 0.5), log, HabitPolicy::default());
        let batch = vec![candidate_named("node_modules")];
        let verdicts = adjudicator.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Safe);
        assert_eq!(verdicts[0].source, VerdictSource::Learned);
        assert!(
            verdicts[0].is_automatically_removable(&ConfidencePolicy::default()),
            "confidence 0.7 meets the learned bar"
        );
        match &verdicts[0].reason {
            Reason::Key { key, .. } => assert_eq!(key, "reason.habituallyRemoved"),
            other => panic!("expected a key, got {other:?}"),
        }
    }

    #[test]
    fn learned_removal_never_overrides_a_structural_keep() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "guarded",
                "userMarked",
                DecisionOutcome::Removed,
                day,
                100,
            ));
        }
        let adjudicator =
            HabitAdjudicator::new(Fixed(Safety::Keep, 0.0), log, HabitPolicy::default());
        let batch = vec![candidate_named("guarded")];
        let verdicts = adjudicator.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Keep);
        assert_eq!(verdicts[0].source, VerdictSource::rule("fixed"));
    }

    #[test]
    fn below_threshold_leaves_the_verdict_alone() {
        let mut log = DecisionLog::new();
        for day in 0..2 {
            log.record(decision(
                "node_modules",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
        }
        let adjudicator =
            HabitAdjudicator::new(Fixed(Safety::Review, 0.5), log, HabitPolicy::default());
        let batch = vec![candidate_named("node_modules")];
        let verdicts = adjudicator.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Review);
        assert_eq!(verdicts[0].source, VerdictSource::rule("fixed"));
    }

    #[test]
    fn an_even_history_changes_nothing() {
        let mut log = DecisionLog::new();
        for day in 0..3 {
            log.record(decision(
                "target",
                "rebuildableCache",
                DecisionOutcome::Removed,
                day,
                100,
            ));
        }
        for day in 3..6 {
            log.record(decision(
                "target",
                "rebuildableCache",
                DecisionOutcome::Kept,
                day,
                100,
            ));
        }
        let adjudicator =
            HabitAdjudicator::new(Fixed(Safety::Review, 0.5), log, HabitPolicy::default());
        let batch = vec![candidate_named("target")];
        let verdicts = adjudicator.adjudicate_batch(&batch, 0).unwrap();
        assert_eq!(verdicts[0].safety, Safety::Review);
        assert_eq!(verdicts[0].source, VerdictSource::rule("fixed"));
    }

    #[test]
    fn name_and_remote_flags_are_forwarded() {
        let adjudicator = HabitAdjudicator::new(
            Fixed(Safety::Review, 0.5),
            DecisionLog::new(),
            HabitPolicy::default(),
        );
        assert_eq!(adjudicator.name(), "fixed");
        assert!(!adjudicator.is_remote());
    }
}
