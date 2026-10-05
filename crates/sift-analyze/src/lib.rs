//! Sift analysis: what could be cleaned, how safe it is, and why.
//!
//! The pipeline has three deliberate stages, in this order:
//!
//! 1. **[`rules`] nominate candidates.** Local, deterministic, auditable, free.
//!    Nothing else may originate a candidate, which also decides what a remote
//!    service is allowed to learn about.
//! 2. **[`adjudicate`] turns nominations into verdicts**, attaching a safety
//!    level, a confidence, and a reason. [`adjudicate::RuleAdjudicator`] does
//!    this offline; a remote provider can confirm or soften a nomination but
//!    can never invent a `Safe` on its own.
//! 3. **[`route`] decides what may leave the machine.** Structural families
//!    (caches, build output, trash) are resolved locally and never transmitted;
//!    only families whose safety depends on content are sent, and only with
//!    redacted paths.
//!
//! The crate is pure: it reads the filesystem for the bounded file pass, but it
//! has no storage, no network, and no UI. Persistence is [`crate::adjudicate::VerdictCache`]'s
//! contract, implemented elsewhere; the remote client is behind the
//! `remote-ai` feature.

pub mod adjudicate;
pub mod analyzer;
pub mod candidate;
pub mod cleanup;
pub mod habits;
pub mod prompt;
pub mod reason;
pub mod route;
pub mod simulate;
pub mod rules;

#[cfg(feature = "remote-ai")]
pub mod remote;

pub use adjudicate::{
    adjudicate_all, apply_remote_verdicts, parse_raw_verdicts, sanitize_model_verdict,
    AdjudicateError, Adjudicator, AnalysisReport, AnalyzedItem, CachedAdjudicator, Downgrade,
    Guardrails, RawVerdict, RuleAdjudicator, VerdictCache,
};
pub use analyzer::{AnalysisPolicy, AnalysisStage, Analyzer};
pub use candidate::{redact_path, Candidate, CandidateKind, Evidence, Nomination};
pub use cleanup::{
    display_command, plan_for, CleanupMethod, CleanupPlan, CommandStep,
};
pub use habits::{
    Decision, DecisionLog, DecisionOutcome, HabitAdjudicator, HabitPolicy, LEARNED_CONFIDENCE,
    RoutineSuggestion, SuggestionReason,
};
pub use reason::{
    clamp_confidence, ConfidencePolicy, PathFingerprint, Reason, Safety, Verdict, VerdictSource,
};
pub use route::{RemoteAdjudicator, RoutingAdjudicator};
pub use prompt::{build as build_prompt_shared, build_at as build_prompt_at_shared, PromptItem};
pub use simulate::SimulatedAdjudicator;
pub use rules::{RuleHit, RuleThresholds};

#[cfg(feature = "remote-ai")]
pub use remote::{LlmConfig, OpenAiCompatibleAdjudicator, RetrySignals};
