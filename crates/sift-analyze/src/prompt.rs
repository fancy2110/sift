//! Versioned adjudication prompt.
//!
//! Pure string construction with no network dependency, so it compiles with or
//! without the `remote-ai` feature and is shared by the real remote client and
//! the offline [`crate::mock::MockAdjudicator`]. That sharing is what makes the
//! prompt genuinely reusable: one template, verified in tests for the privacy
//! contract, used by every provider.

use serde::Serialize;

use crate::candidate::Candidate;

/// Version of the prompt/contract below. Bump on any prompt or output-schema
/// change so cached verdicts and recorded prompt versions stay traceable.
pub const PROMPT_VERSION: u32 = 2;

/// One item as serialized into the user message. Paths are display-only (home
/// collapsed); no absolute path appears here.
#[derive(Debug, Serialize)]
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
Reply with ONLY a JSON array. Each element:\n\
{{\"id\": string, \"safety\": \"safe\"|\"review\"|\"keep\", \"confidence\": number 0..1, \
\"reason\": string, \"impact\": {{\"summary\": string, \"reversible\": boolean, \
\"warnings\": [string]}}, \"commands\": [{{\"command\": string, \
\"platform\": \"macos\"|\"linux\"|\"windows\"|\"any\", \"effect\": string}}]}}.\n\
Rules:\n\
- \"safe\" means deleting it loses nothing the user cannot restore or regenerate.\n\
- \"review\" means a human must decide. \"keep\" means it should not be deleted.\n\
- If you are unsure, answer \"review\". Never guess \"safe\".\n\
- \"reason\" is ONE short sentence in {language}, naming the object and why. \
It is shown verbatim in the interface.\n\
- \"impact.summary\" is one sentence in {language} on what depends on this object; \
set \"reversible\" honestly; list at most three concrete pre-delete checks.\n\
- Suggest commands ONLY when there is a genuinely useful native/dedicated \
cleanup (emptying a cache, a toolchain's own clean command). The command must \
target only this object. Suggest nothing for ordinary files.\n\
- Judge only what you are given. You cannot see file contents; do not imply that you did.\n\
- Return one element per item, using the exact ids given.",
        language = language
    );

    let items: Vec<PromptCandidate<'_>> = batch
        .iter()
        .map(|candidate| PromptCandidate {
            id: candidate.key.to_string(),
            path: candidate.display_path.as_str(),
            entry_type: if candidate.is_dir { "directory" } else { "file" },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateKind;
    use sift_core::{ByteSize, NodeKey};

    fn candidate() -> Candidate {
        Candidate::new(
            NodeKey::from_bytes(b"x"),
            std::path::PathBuf::from("/Users/secret/name"),
            "~/name",
            "name",
            false,
            ByteSize::new(100, 100),
            0,
            CandidateKind::StaleLargeFile,
        )
    }

    #[test]
    fn prompt_contract_mentions_impact_and_commands() {
        let (system, _) = build_prompt(&[candidate()], "en");
        assert!(system.contains("impact"));
        assert!(system.contains("commands"));
    }

    #[test]
    fn no_absolute_path_leaks_into_the_user_message() {
        let (_, user) = build_prompt(&[candidate()], "en");
        assert!(user.contains("~/name"));
        assert!(!user.contains("/Users/secret"));
    }
}
