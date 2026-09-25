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

use crate::reason::PathFingerprint;

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
            .filter(|entry| {
                entry.fingerprint.key == key && entry.outcome == DecisionOutcome::Kept
            })
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
    pub fn suggest_routines(&self, min_occurrences: usize, min_days: usize) -> Vec<RoutineSuggestion> {
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
    use sift_core::NodeKey;

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
        log.record(decision("node_modules", "rebuildableCache", DecisionOutcome::Removed, 0, 100));
        log.record(decision("node_modules", "rebuildableCache", DecisionOutcome::Removed, 1, 100));
        log.record(decision("target", "rebuildableCache", DecisionOutcome::Kept, 1, 200));

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
            log.record(decision("small", "archive", DecisionOutcome::Removed, day, 10));
            log.record(decision("large", "archive", DecisionOutcome::Removed, day, 10_000));
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
}
