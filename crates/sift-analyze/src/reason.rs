//! The judgment vocabulary: how safe a removal is, why, and who decided.
//!
//! Three rules shape everything here:
//!
//! 1. **Safety is a closed set of three levels**, and the default for anything
//!    unexamined or uncertain is [`Safety::Review`] — never [`Safety::Safe`].
//!    A bug that loses a verdict must not be able to authorize a deletion.
//! 2. **A reason is data, not a sentence.** Local verdicts carry a stable i18n
//!    key so every locale can phrase it; only a remote adjudicator may return
//!    free text, and that text is marked as such.
//! 3. **A verdict is tied to a fingerprint.** A verdict about a 4 GB cache is
//!    not a verdict about whatever occupies that path next week.

use serde::{Deserialize, Serialize};

use sift_core::id::NodeKey;

/// How consequential removing this entry is.
///
/// Ordered from most to least dangerous so comparisons are meaningful:
/// `Keep` is never eligible for cleanup, `Safe` is the only level automatic
/// cleanup may act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Safety {
    /// Removing it loses data the user would miss. Never offered.
    Keep,
    /// Removing it is plausible but needs a human decision.
    Review,
    /// Rebuildable, regenerable, or already discarded: removing it loses
    /// nothing the user cannot get back.
    Safe,
}

impl Safety {
    /// Whether this level may ever be offered as a cleanup candidate.
    pub const fn is_removable(self) -> bool {
        matches!(self, Safety::Safe | Safety::Review)
    }

    /// Whether automatic (unattended) cleanup may act on it.
    pub const fn is_automatic(self) -> bool {
        matches!(self, Safety::Safe)
    }

    /// i18n key for the level's label.
    pub const fn label_key(self) -> &'static str {
        match self {
            Safety::Keep => "safety.keep",
            Safety::Review => "safety.review",
            Safety::Safe => "safety.safe",
        }
    }

    /// The level to use when a judgment is missing, malformed, or rejected.
    ///
    /// This is deliberately `Review`, not the most conservative `Keep`: an
    /// unjudged candidate should still be *visible* to the user, but must never
    /// be automatically removable.
    pub const fn fallback() -> Self {
        Safety::Review
    }
}

/// Why something is considered cleanable, phrased for the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Reason {
    /// A stable i18n key plus positional parameters, rendered by the front end.
    /// Preferred: it keeps every locale in control of word order.
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        params: Vec<String>,
    },
    /// Free text produced by a remote adjudicator, already written in the
    /// user's language. Never produced by local rules.
    Text(String),
}

impl Reason {
    pub fn key(key: impl Into<String>) -> Self {
        Reason::Key {
            key: key.into(),
            params: Vec::new(),
        }
    }

    pub fn key_with(key: impl Into<String>, params: Vec<String>) -> Self {
        Reason::Key {
            key: key.into(),
            params,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Reason::Text(text.into())
    }

    /// Whether this reason can be shown to a user without further lookup.
    pub fn is_text(&self) -> bool {
        matches!(self, Reason::Text(_))
    }

    /// Short human summary for logs and tests; the UI renders `Key` through its
    /// translation table instead.
    pub fn describe(&self) -> String {
        match self {
            Reason::Key { key, params } if params.is_empty() => key.clone(),
            Reason::Key { key, params } => format!("{key}({})", params.join(", ")),
            Reason::Text(text) => text.clone(),
        }
    }

    /// Clamp remote free text to something a row can display, without newlines
    /// or runaway length. Returns `None` when nothing usable remains.
    pub fn sanitized_text(text: &str) -> Option<Reason> {
        let cleaned: String = text
            .chars()
            .map(|ch| {
                if ch == '\n' || ch == '\r' || ch == '\t' {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
        let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        let trimmed = collapsed.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut out: String = trimmed.chars().take(240).collect();
        if trimmed.chars().count() > 240 {
            out.push('…');
        }
        Some(Reason::Text(out))
    }
}

/// Who produced a verdict. Kept explicit so policy can treat sources
/// differently — an unattended cleanup should weigh a rule and a language model
/// differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VerdictSource {
    /// A local, offline rule. Deterministic and auditable.
    Rule { rule: String },
    /// Reused from the local verdict store (a previous judgment of the same
    /// fingerprint).
    Cached,
    /// Inferred from the user's own repeated decisions.
    Learned,
    /// The user decided this explicitly.
    UserDecision,
    /// A remote adjudicator (language model).
    Remote { provider: String },
}

impl VerdictSource {
    pub fn rule(rule: impl Into<String>) -> Self {
        VerdictSource::Rule { rule: rule.into() }
    }

    pub fn remote(provider: impl Into<String>) -> Self {
        VerdictSource::Remote {
            provider: provider.into(),
        }
    }

    /// Whether this source is a language model's opinion rather than a local
    /// deterministic judgment.
    pub fn is_model(&self) -> bool {
        matches!(self, VerdictSource::Remote { .. })
    }

    /// Whether this conclusion came from the user (explicitly or by habit).
    pub fn is_user_derived(&self) -> bool {
        matches!(self, VerdictSource::UserDecision | VerdictSource::Learned)
    }

    pub fn label(&self) -> String {
        match self {
            VerdictSource::Rule { rule } => format!("rule:{rule}"),
            VerdictSource::Cached => "cached".to_string(),
            VerdictSource::Learned => "learned".to_string(),
            VerdictSource::UserDecision => "user".to_string(),
            VerdictSource::Remote { provider } => format!("ai:{provider}"),
        }
    }
}

/// A judgment about one candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Verdict {
    pub safety: Safety,
    /// `0.0..=1.0`. Clamped on construction; a missing or nonsensical value
    /// becomes `0.0`, which can never satisfy a confidence threshold.
    pub confidence: f32,
    pub reason: Reason,
    pub source: VerdictSource,
    /// Unix milliseconds when the judgment was made.
    pub judged_at_ms: i64,
}

impl Verdict {
    pub fn new(
        safety: Safety,
        confidence: f32,
        reason: Reason,
        source: VerdictSource,
        judged_at_ms: i64,
    ) -> Self {
        Self {
            safety,
            confidence: clamp_confidence(confidence),
            reason,
            source,
            judged_at_ms,
        }
    }

    /// The verdict used whenever a judgment is absent or unusable: visible, but
    /// never automatically removable.
    pub fn unknown(judged_at_ms: i64) -> Self {
        Self {
            safety: Safety::fallback(),
            confidence: 0.0,
            reason: Reason::key("reason.unjudged"),
            source: VerdictSource::Rule {
                rule: "fallback".into(),
            },
            judged_at_ms,
        }
    }

    /// Whether an unattended cleanup may act on this verdict under `policy`.
    pub fn is_automatically_removable(&self, policy: &ConfidencePolicy) -> bool {
        self.safety.is_automatic() && self.confidence >= policy.minimum_for_source(&self.source)
    }
}

/// NaN-safe confidence clamp: everything outside `0.0..=1.0` becomes `0.0`, so
/// a malformed value can never look confident.
pub fn clamp_confidence(value: f32) -> f32 {
    if value.is_nan() || value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

/// How much confidence each kind of source needs before it may authorize an
/// unattended removal.
///
/// A local rule fires only on a pattern it was written for, so it needs less
/// evidence than a language model, which can be wrong in ways rules cannot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ConfidencePolicy {
    pub rule: f32,
    pub cached: f32,
    pub learned: f32,
    pub user: f32,
    pub remote: f32,
}

impl Default for ConfidencePolicy {
    fn default() -> Self {
        Self {
            rule: 0.6,
            cached: 0.6,
            learned: 0.7,
            user: 0.0,
            // A model must be quite sure before it can drive an unattended
            // deletion.
            remote: 0.85,
        }
    }
}

impl ConfidencePolicy {
    pub fn minimum_for_source(&self, source: &VerdictSource) -> f32 {
        match source {
            VerdictSource::Rule { .. } => self.rule,
            VerdictSource::Cached => self.cached,
            VerdictSource::Learned => self.learned,
            VerdictSource::UserDecision => self.user,
            VerdictSource::Remote { .. } => self.remote,
        }
    }
}

/// What makes a verdict reusable later.
///
/// Identity alone is not enough: the entry at a path can be replaced by a
/// different, larger thing. Size and modification time must match too, so a
/// verdict about a 4 GB cache cannot authorize deleting whatever appears there
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PathFingerprint {
    pub key: NodeKey,
    /// Logical size at the time of judgment.
    pub size: u64,
    /// Modification time in unix milliseconds at the time of judgment.
    pub mtime_ms: i64,
    /// Whether the entry was a directory; a directory verdict must not be
    /// applied to a file that later replaces it.
    pub is_dir: bool,
}

impl PathFingerprint {
    pub fn new(key: NodeKey, size: u64, mtime_ms: i64, is_dir: bool) -> Self {
        Self {
            key,
            size,
            mtime_ms,
            is_dir,
        }
    }

    /// Whether a stored verdict for `self` still describes `other`.
    ///
    /// Deliberately strict: any difference invalidates the judgment. A cache
    /// directory that changed size is no longer the cache that was judged.
    pub fn still_describes(&self, other: &PathFingerprint) -> bool {
        self.key == other.key
            && self.size == other.size
            && self.mtime_ms == other.mtime_ms
            && self.is_dir == other.is_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(size: u64, mtime: i64) -> PathFingerprint {
        PathFingerprint::new(NodeKey::from_bytes(b"/x"), size, mtime, true)
    }

    #[test]
    fn safety_orders_keep_first_safe_last() {
        assert!(Safety::Keep < Safety::Review);
        assert!(Safety::Review < Safety::Safe);
        assert!(!Safety::Keep.is_removable());
        assert!(Safety::Review.is_removable());
        assert!(!Safety::Review.is_automatic());
        assert!(Safety::Safe.is_automatic());
    }

    #[test]
    fn missing_judgment_is_never_automatic() {
        let unknown = Verdict::unknown(1);
        assert_eq!(unknown.safety, Safety::Review);
        assert!(!unknown.safety.is_automatic());
        assert!(!unknown.is_automatically_removable(&ConfidencePolicy::default()));
    }

    #[test]
    fn confidence_is_clamped_and_nan_safe() {
        assert_eq!(clamp_confidence(f32::NAN), 0.0);
        assert_eq!(clamp_confidence(-1.0), 0.0);
        assert_eq!(clamp_confidence(1.5), 1.0);
        assert_eq!(clamp_confidence(0.42), 0.42);
        let verdict = Verdict::new(
            Safety::Safe,
            f32::NAN,
            Reason::key("k"),
            VerdictSource::rule("r"),
            0,
        );
        assert_eq!(verdict.confidence, 0.0);
    }

    #[test]
    fn remote_source_needs_more_confidence_than_a_rule() {
        let policy = ConfidencePolicy::default();
        let rule = Verdict::new(
            Safety::Safe,
            0.7,
            Reason::key("k"),
            VerdictSource::rule("cache"),
            0,
        );
        let model = Verdict::new(
            Safety::Safe,
            0.7,
            Reason::text("looks like a cache"),
            VerdictSource::remote("test"),
            0,
        );
        assert!(rule.is_automatically_removable(&policy));
        assert!(
            !model.is_automatically_removable(&policy),
            "0.7 confidence from a model must not be enough"
        );
    }

    #[test]
    fn user_decision_needs_no_confidence() {
        let policy = ConfidencePolicy::default();
        let verdict = Verdict::new(
            Safety::Safe,
            0.0,
            Reason::key("k"),
            VerdictSource::UserDecision,
            0,
        );
        assert!(verdict.is_automatically_removable(&policy));
    }

    #[test]
    fn free_text_is_sanitized_to_one_line() {
        let reason = Reason::sanitized_text("  a\nb\tc   d  ").expect("usable");
        assert_eq!(reason.describe(), "a b c d");
        assert!(Reason::sanitized_text("   \n  ").is_none());
        let long = "x".repeat(500);
        let reason = Reason::sanitized_text(&long).unwrap();
        match reason {
            Reason::Text(text) => {
                assert!(text.chars().count() <= 241);
                assert!(text.ends_with('…'));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn i18n_keys_survive_a_round_trip() {
        let reason = Reason::key_with("reason.cache.nodeModules", vec!["npm".into()]);
        let json = serde_json::to_string(&reason).unwrap();
        let back: Reason = serde_json::from_str(&json).unwrap();
        assert_eq!(reason, back);
        assert_eq!(back.describe(), "reason.cache.nodeModules(npm)");
        assert!(!back.is_text());
    }

    #[test]
    fn fingerprint_requires_every_field_to_match() {
        let base = fingerprint(100, 1000);
        assert!(base.still_describes(&fingerprint(100, 1000)));
        assert!(
            !base.still_describes(&fingerprint(101, 1000)),
            "size changed"
        );
        assert!(
            !base.still_describes(&fingerprint(100, 1001)),
            "mtime changed"
        );
        let as_file = PathFingerprint::new(NodeKey::from_bytes(b"/x"), 100, 1000, false);
        assert!(!base.still_describes(&as_file), "kind changed");
    }

    #[test]
    fn verdict_serde_round_trip() {
        let verdict = Verdict::new(
            Safety::Safe,
            0.9,
            Reason::key("k"),
            VerdictSource::remote("ark"),
            123,
        );
        let json = serde_json::to_string(&verdict).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(verdict, back);
        assert!(back.source.is_model());
        assert_eq!(back.source.label(), "ai:ark");
    }
}
