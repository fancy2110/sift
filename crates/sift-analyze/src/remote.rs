//! Remote adjudication through an OpenAI-compatible chat-completions endpoint.
//!
//! Compiled only with the `remote-ai` feature, and inert until a caller both
//! configures an endpoint and flips the consent switch — a network client that
//! can see a file list should never be on by accident.
//!
//! What is sent, and what is not:
//!
//! * **Sent:** the redacted path (`~/Downloads/x.dmg`), whether it is a
//!   directory, its size, its age in days, its family, and the rule signals
//!   that nominated it.
//! * **Never sent:** the real absolute path, the API key's owner's other
//!   candidates, file contents, or anything about candidates whose family is
//!   not model-adjudicable (the router removes those before this code runs).
//!
//! The response is validated by [`crate::adjudicate::apply_remote_verdicts`]
//! before it can influence anything, so a malformed or adversarial answer can
//! only ever produce `Review`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::adjudicate::{apply_remote_verdicts, parse_raw_verdicts, AdjudicateError, Guardrails};
use crate::candidate::Candidate;
use crate::reason::Verdict;
use crate::route::RemoteAdjudicator;

/// Live flags polled while a rate-limited request is backing off.
///
/// The default back-off after a rate limit can run several hours (up to
/// [`MAX_RETRY_ATTEMPTS`] sleeps). Without these flags nothing could interrupt
/// that sleep: cancelling the run or withdrawing AI consent in settings would
/// not take effect until the retries were exhausted. Clone the handle and flip
/// it from another thread.
#[derive(Clone)]
pub struct RetrySignals {
    cancel: Arc<AtomicBool>,
    consent: Arc<AtomicBool>,
}

impl Default for RetrySignals {
    /// A standalone set with consent granted, for callers that never flip
    /// either flag.
    fn default() -> Self {
        Self::new(true)
    }
}

impl RetrySignals {
    /// Start with a specific consent state.
    pub fn new(consent: bool) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            consent: Arc::new(AtomicBool::new(consent)),
        }
    }

    /// Ask the waiting loop to give up.
    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Withdraw the consent the wait was granted under.
    pub fn withdraw_consent(&self) {
        self.consent.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn consent_granted(&self) -> bool {
        self.consent.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Connection and consent settings for a remote adjudicator.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LlmConfig {
    /// Provider label recorded in verdict sources, e.g. `ark`, `openai`.
    pub provider: String,
    /// Full chat-completions URL.
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub timeout: Duration,
    /// Candidates per request. Smaller batches are easier for a model to answer
    /// consistently and easier to retry.
    pub batch_size: usize,
    /// Language the model should write its reasons in, as a BCP-47 tag.
    pub language: String,
    /// Whether the user has explicitly agreed to send file metadata off the
    /// machine. [`LlmConfig::new`] starts at `false`.
    pub consent: bool,
}

impl LlmConfig {
    /// A configured but **not consented** provider. Every field is required
    /// because a half-configured provider should fail loudly, not degrade into
    /// odd answers.
    pub fn new(
        provider: impl Into<String>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            endpoint: endpoint.into(),
            model: model.into(),
            api_key: api_key.into(),
            timeout: Duration::from_secs(45),
            batch_size: 30,
            language: "zh".to_string(),
            consent: false,
        }
    }

    /// Record the user's explicit agreement to transmit file metadata.
    pub fn with_consent(mut self) -> Self {
        self.consent = true;
        self
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.max(1);
        self
    }

    /// Whether this configuration may actually be used.
    pub fn is_usable(&self) -> bool {
        self.consent
            && !self.endpoint.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.api_key.trim().is_empty()
    }
}

/// Rate-limit retries before giving up. With the half-hour default back-off,
/// nine attempts span roughly four hours.
const MAX_RETRY_ATTEMPTS: usize = 9;

/// How a rate-limit wait ended.
enum WaitOutcome {
    /// The full back-off elapsed; the caller may retry.
    Elapsed,
    /// The cancel flag was flipped during the wait.
    Cancelled,
    /// Consent was withdrawn during the wait.
    ConsentWithdrawn,
}

/// One candidate as the model sees it.
///
/// A dedicated struct rather than serializing [`Candidate`] directly, so that
/// adding a field to the local model can never accidentally add it to the
/// payload.
#[derive(serde::Serialize)]
struct PromptCandidate<'a> {
    id: String,
    path: &'a str,
    entry_type: &'static str,
    size_bytes: u64,
    age_days: u64,
    family: &'static str,
    signals: Vec<&'a str>,
}

/// Build the system and user messages for one batch.
///
/// Pure and public so the payload can be asserted in tests: that is the only
/// practical way to prove no absolute path leaks.
pub fn build_prompt(batch: &[Candidate], language: &str) -> (String, String) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    build_prompt_at(batch, language, now_ms)
}

/// [`build_prompt`] with an explicit clock, for reproducible tests.
pub fn build_prompt_at(batch: &[Candidate], language: &str, now_ms: i64) -> (String, String) {
    let system = format!(
        "You are a conservative disk-cleanup reviewer inside a desktop app. \
For each item you receive, decide whether it can be deleted.\n\
Reply with ONLY a JSON array. Each element: {{\"id\": string, \"safety\": \"safe\"|\"review\"|\"keep\", \"confidence\": number 0..1, \"reason\": string, \"impact\": string}}.\n\
Rules:\n\
- \"safe\" means deleting it loses nothing the user cannot restore or regenerate.\n\
- \"review\" means a human must decide. \"keep\" means it should not be deleted.\n\
- If you are unsure, answer \"review\". Never guess \"safe\".\n\
- \"reason\" is ONE short sentence in {language}, naming the object and why.\n\
- \"impact\" is ONE short sentence in {language} describing what breaks or is \
regenerated after deletion (e.g. which app must rebuild, which data is unrecoverable).\n\
- Do not propose shell commands: the app attaches the correct cleanup command.\n\
- Judge only what you are given. You cannot see file contents; do not imply that you did.\n\
- Return one element per item, using the exact ids given.",
        language = language
    );

    let items: Vec<PromptCandidate<'_>> = batch
        .iter()
        .map(|candidate| PromptCandidate {
            id: candidate.key.to_string(),
            path: candidate.display_path.as_str(),
            entry_type: if candidate.is_dir {
                "directory"
            } else {
                "file"
            },
            size_bytes: candidate.size.dominant(),
            age_days: crate::rules::age_days(candidate.mtime_ms, now_ms),
            family: candidate.kind.token(),
            signals: candidate
                .evidence
                .iter()
                .map(|evidence| evidence.rule.as_str())
                .collect(),
        })
        .collect();

    let user = format!(
        "Items to review:\n{}",
        serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
    );
    (system, user)
}

/// A remote adjudicator backed by a chat-completions endpoint.
pub struct OpenAiCompatibleAdjudicator {
    config: LlmConfig,
    client: reqwest::blocking::Client,
    signals: RetrySignals,
}

impl OpenAiCompatibleAdjudicator {
    /// Build a client. Fails when consent is missing: constructing the
    /// adjudicator is the point of no return, so it is refused there rather
    /// than at request time.
    pub fn new(config: LlmConfig) -> Result<Self, AdjudicateError> {
        if !config.consent {
            return Err(AdjudicateError::NotConfigured(
                "remote adjudication requires explicit consent".to_string(),
            ));
        }
        if !config.is_usable() {
            return Err(AdjudicateError::NotConfigured(
                "endpoint, model and API key are all required".to_string(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|err| AdjudicateError::Transport(err.to_string()))?;
        Ok(Self {
            config,
            client,
            signals: RetrySignals::default(),
        })
    }

    /// Attach externally visible cancel/consent flags polled during the
    /// rate-limit back-off. Without this the client uses an internal set that
    /// nothing else can flip.
    pub fn with_retry_signals(mut self, signals: RetrySignals) -> Self {
        self.signals = signals;
        self
    }

    /// A handle to this client's live retry flags.
    pub fn retry_signals(&self) -> RetrySignals {
        self.signals.clone()
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// How many candidates to send per request.
    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    /// Send `batch` in sub-batches and validate every answer.
    fn request_batch(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        // Rate limits are not a failed analysis: wait and retry.
        let mut attempt = 1usize;
        loop {
            match self.request_batch_once(batch, guardrails, now_ms) {
                Ok(verdicts) => return Ok(verdicts),
                Err(Retry::RateLimited(wait_ms)) if attempt < MAX_RETRY_ATTEMPTS => {
                    attempt += 1;
                    // The wait can be cut short by cancellation or by consent
                    // withdrawal; a retry never proceeds on stale permission.
                    match self.wait_for_retry(wait_ms) {
                        WaitOutcome::Elapsed => {}
                        WaitOutcome::Cancelled => return Err(AdjudicateError::Cancelled),
                        WaitOutcome::ConsentWithdrawn => {
                            return Err(AdjudicateError::NotConfigured(
                                "remote consent withdrawn during retry".to_string(),
                            ))
                        }
                    }
                }
                Err(Retry::RateLimited(_)) => {
                    return Err(AdjudicateError::Transport(
                        "rate limit persisted after retries".to_string(),
                    ))
                }
                Err(Retry::Fatal(message)) => {
                    return Err(AdjudicateError::Transport(message))
                }
            }
        }
    }

    /// Sleep up to `wait_ms` in short slices, polling the live cancel/consent
    /// flags so interruption latency is bounded by one slice.
    fn wait_for_retry(&self, wait_ms: u64) -> WaitOutcome {
        /// Poll this often; cancellation is observed within this long.
        const POLL_SLICE: Duration = Duration::from_millis(200);
        let mut remaining = wait_ms;
        loop {
            if self.signals.cancelled() {
                return WaitOutcome::Cancelled;
            }
            if !self.signals.consent_granted() {
                return WaitOutcome::ConsentWithdrawn;
            }
            if remaining == 0 {
                return WaitOutcome::Elapsed;
            }
            let step = remaining.min(POLL_SLICE.as_millis() as u64);
            std::thread::sleep(Duration::from_millis(step));
            remaining -= step;
        }
    }

    /// One attempt at a batch.
    fn request_batch_once(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<Verdict>, Retry> {
        let (system, user) = build_prompt_at(batch, &self.config.language, now_ms);
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "temperature": 0,
            "response_format": { "type": "json_object" },
        });

        let response = self
            .client
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .map_err(|err| {
                if is_rate_limit_error(&err) {
                    Retry::RateLimited(DEFAULT_RETRY_WAIT_MS)
                } else {
                    Retry::Fatal(err.to_string())
                }
            })?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        let text = response
            .text()
            .map_err(|err| Retry::Fatal(err.to_string()))?;
        if !status.is_success() {
            let code = status.as_u16();
            let lower = text.to_ascii_lowercase();
            let mentions_quota = [
                "rate limit",
                "rate_limit",
                "quota",
                "too many requests",
            ]
            .iter()
            .any(|needle| lower.contains(needle));
            if code == 429 || code == 503 || mentions_quota {
                return Err(Retry::RateLimited(
                    retry_after.unwrap_or(DEFAULT_RETRY_WAIT_MS),
                ));
            }
            return Err(Retry::Fatal(format!(
                "HTTP {}: {}",
                code,
                text.chars().take(200).collect::<String>()
            )));
        }

        let content =
            extract_content(&text).map_err(|err| Retry::Fatal(err.to_string()))?;
        let raw = parse_raw_verdicts(&content).map_err(|err| Retry::Fatal(err.to_string()))?;
        Ok(apply_remote_verdicts(
            batch,
            &raw,
            guardrails,
            now_ms,
            &self.config.provider,
        ))
    }
}

/// Outcome of one request attempt.
enum Retry {
    /// Provider asked to slow down; back-off in milliseconds.
    RateLimited(u64),
    /// A non-rate-limit failure.
    Fatal(String),
}

/// Half an hour, the mandated default back-off for token/rate-limit problems.
const DEFAULT_RETRY_WAIT_MS: u64 = 30 * 60 * 1000;

/// Parse a `Retry-After` value: HTTP-date or delta-seconds. Only a bounded
/// delta-seconds value is accepted; an HTTP date is ignored here rather than
/// trusted with clock math.
fn parse_retry_after(value: &str) -> Option<u64> {
    let secs: u64 = value.trim().parse().ok()?;
    // Cap a single wait at 1 h so a header typo cannot park the app for days.
    Some(secs.clamp(1, 3600) * 1000)
}

/// Whether a connection-level error reads as a rate limit.
fn is_rate_limit_error(err: &reqwest::Error) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    ["429", "rate limit", "rate_limit", "too many requests", "quota"]
        .iter()
        .any(|needle| text.contains(needle))
}

impl RemoteAdjudicator for OpenAiCompatibleAdjudicator {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn adjudicate_remote(
        &self,
        batch: &[Candidate],
        guardrails: &Guardrails,
        now_ms: i64,
    ) -> Result<Vec<Verdict>, AdjudicateError> {
        let mut out = Vec::with_capacity(batch.len());
        for chunk in batch.chunks(self.config.batch_size.max(1)) {
            out.extend(self.request_batch(chunk, guardrails, now_ms)?);
        }
        Ok(out)
    }
}

/// Pull the assistant message text out of a chat-completions reply.
fn extract_content(body: &str) -> Result<String, AdjudicateError> {
    #[derive(serde::Deserialize)]
    struct Reply {
        choices: Vec<Choice>,
    }
    #[derive(serde::Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(serde::Deserialize)]
    struct Message {
        content: String,
    }

    let reply: Reply = serde_json::from_str(body)
        .map_err(|err| AdjudicateError::Response(format!("unrecognised reply: {err}")))?;
    reply
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| AdjudicateError::Response("reply contained no choices".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{CandidateKind, Evidence};
    use crate::reason::{Safety, VerdictSource};
    use sift_core::{ByteSize, NodeKey};
    use std::path::PathBuf;

    fn candidate(kind: CandidateKind, rule: &str, display: &str, age_days: i64) -> Candidate {
        let mut candidate = Candidate::new(
            NodeKey::from_bytes(rule.as_bytes()),
            // A real absolute path that must never appear in the payload.
            PathBuf::from("/Users/realuser/秘密/project").join(display),
            display,
            display,
            false,
            ByteSize::new(1 << 30, 1 << 30),
            86_400_000 * age_days,
            kind,
        );
        candidate.evidence.push(Evidence::new(rule, "detail.x"));
        candidate
    }

    #[test]
    fn configuration_requires_consent() {
        let config = LlmConfig::new("ark", "https://example.test/v1/chat", "m", "key");
        assert!(!config.is_usable(), "no consent yet");
        let config = config.with_consent();
        assert!(config.is_usable());
        // Constructing without consent is refused outright.
        let without = LlmConfig::new("ark", "https://example.test/v1/chat", "m", "key");
        assert!(OpenAiCompatibleAdjudicator::new(without).is_err());
    }

    #[test]
    fn incomplete_configuration_is_refused() {
        let config = LlmConfig::new("ark", "", "m", "key").with_consent();
        assert!(!config.is_usable());
        assert!(OpenAiCompatibleAdjudicator::new(config).is_err());
    }

    #[test]
    fn prompt_contains_only_redacted_paths() {
        let batch = vec![candidate(
            CandidateKind::StaleLargeFile,
            "file.stale_large",
            "~/Downloads/old.iso",
            400,
        )];
        let (system, user) = build_prompt_at(&batch, "zh", 86_400_000 * 500);

        assert!(system.contains("review"));
        assert!(system.contains("zh"));
        assert!(user.contains("~/Downloads/old.iso"));
        assert!(user.contains("staleLargeFile"));
        assert!(user.contains("100"), "age should be computed: {user}");

        // The privacy property: no absolute path, no user name, no home dir.
        for leak in ["/Users/", "realuser", "秘密", "/Users/realuser"] {
            assert!(!user.contains(leak), "payload leaked {leak:?}: {user}");
            assert!(!system.contains(leak));
        }
    }

    #[test]
    fn prompt_payload_is_valid_json_with_the_expected_fields() {
        let batch = vec![
            candidate(
                CandidateKind::Archive {
                    extension: "zip".into(),
                },
                "file.archive",
                "~/a.zip",
                10,
            ),
            candidate(
                CandidateKind::StaleLargeFile,
                "file.stale_large",
                "~/b.iso",
                20,
            ),
        ];
        let (_, user) = build_prompt_at(&batch, "en", 86_400_000 * 30);
        let json = user
            .split_once('\n')
            .map(|(_, rest)| rest)
            .unwrap_or(user.as_str());
        let parsed: Vec<serde_json::Value> = serde_json::from_str(json.trim()).expect("valid JSON");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["entry_type"], "file");
        assert_eq!(parsed[0]["family"], "archive");
        assert_eq!(parsed[1]["age_days"], 10);
        assert!(parsed[0]["signals"].as_array().unwrap().len() == 1);
        // The id must be the stable key the answer has to quote back.
        assert!(parsed[0]["id"].as_str().unwrap().starts_with("n-"));
    }

    #[test]
    fn reply_extraction_reports_unusable_shapes() {
        let ok = r#"{"choices":[{"message":{"content":"[]"}}]}"#;
        assert_eq!(extract_content(ok).unwrap(), "[]");
        assert!(extract_content("{}").is_err());
        assert!(extract_content("not json").is_err());
        assert!(extract_content(r#"{"choices":[]}"#).is_err());
    }

    /// The end-to-end guardrail: a model answering `safe` for a family it may
    /// not authorize still yields `Review`, through the same code path the real
    /// client uses.
    #[test]
    fn a_model_cannot_authorize_structural_families_through_the_real_path() {
        use crate::adjudicate::RawVerdict;
        let trash = candidate(CandidateKind::Trash, "dir.trash", "~/.Trash", 0);
        let raw = vec![RawVerdict {
            id: trash.key.to_string(),
            safety: "safe".into(),
            confidence: 1.0,
            reason: "empty it".into(),
            impact: String::new(),
        }];
        let verdicts = apply_remote_verdicts(&[trash], &raw, &Guardrails::default(), 0, "remote");
        assert_eq!(verdicts[0].safety, Safety::Review);
        assert!(verdicts[0].source.is_model());
        assert_eq!(verdicts[0].source, VerdictSource::remote("remote"));
    }

    fn new_test_client() -> OpenAiCompatibleAdjudicator {
        let config =
            LlmConfig::new("ark", "https://example.test/v1/chat", "m", "key").with_consent();
        OpenAiCompatibleAdjudicator::new(config).unwrap()
    }

    #[test]
    fn retry_wait_returns_elapsed_for_an_uninterrupted_backoff() {
        let adjudicator = new_test_client();
        assert!(matches!(
            adjudicator.wait_for_retry(10),
            WaitOutcome::Elapsed
        ));
    }

    #[test]
    fn retry_wait_is_interruptible_and_rechecks_consent() {
        // A minute-long back-off must end within milliseconds when the cancel
        // flag flips, rather than sleeping the full wait.
        let adjudicator = new_test_client();
        let signals = adjudicator.retry_signals();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            signals.cancel();
        });
        let started = std::time::Instant::now();
        assert!(matches!(
            adjudicator.wait_for_retry(60_000),
            WaitOutcome::Cancelled
        ));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancel must cut the wait short"
        );
        handle.join().unwrap();

        // Withdrawing consent while asleep ends it with the consent outcome.
        let adjudicator = new_test_client();
        let signals = adjudicator.retry_signals();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            signals.withdraw_consent();
        });
        assert!(matches!(
            adjudicator.wait_for_retry(60_000),
            WaitOutcome::ConsentWithdrawn
        ));
        handle.join().unwrap();
    }
}
