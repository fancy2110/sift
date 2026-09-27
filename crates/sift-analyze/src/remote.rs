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

use std::time::Duration;

use crate::adjudicate::{apply_remote_verdicts, parse_raw_verdicts, AdjudicateError, Guardrails};
use crate::candidate::Candidate;
use crate::reason::Verdict;
use crate::route::RemoteAdjudicator;

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
Reply with ONLY a JSON array. Each element: \
{{\"id\": string, \"safety\": \"safe\"|\"review\"|\"keep\", \"confidence\": number 0..1, \"reason\": string}}.\n\
Rules:\n\
- \"safe\" means deleting it loses nothing the user cannot restore or regenerate.\n\
- \"review\" means a human must decide. \"keep\" means it should not be deleted.\n\
- If you are unsure, answer \"review\". Never guess \"safe\".\n\
- \"reason\" is ONE short sentence in {language}, naming the object and why. \
It is shown verbatim in the interface.\n\
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
        Ok(Self { config, client })
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
            .map_err(|err| AdjudicateError::Transport(err.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|err| AdjudicateError::Transport(err.to_string()))?;
        if !status.is_success() {
            // The body may contain a provider error; keep it short and never
            // echo the key.
            return Err(AdjudicateError::Transport(format!(
                "HTTP {}: {}",
                status.as_u16(),
                text.chars().take(200).collect::<String>()
            )));
        }

        let content = extract_content(&text)?;
        let raw = parse_raw_verdicts(&content)?;
        Ok(apply_remote_verdicts(batch, &raw, guardrails, now_ms))
    }
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
        }];
        let verdicts = apply_remote_verdicts(&[trash], &raw, &Guardrails::default(), 0);
        assert_eq!(verdicts[0].safety, Safety::Review);
        assert!(verdicts[0].source.is_model());
        assert_eq!(verdicts[0].source, VerdictSource::remote("remote"));
    }
}
