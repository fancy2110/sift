//! The reusable adjudication prompt, shared by every model backend.
//!
//! Kept in its own always-compiled module so three things use one contract:
//!
//! * the real OpenAI-compatible network client (`remote-ai`);
//! * the offline [`crate::simulate::SimulatedAdjudicator`], which stands in for
//!   a model when no API key is configured;
//! * tests that prove exactly what leaves the machine.
//!
//! The model returns a judgment plus an *impact*. The concrete cleanup command
//! is deliberately not asked for: [`crate::cleanup`] attaches the correct,
//! toolchain-specific command deterministically, which is safer than trusting a
//! model to invent shell.

use crate::candidate::Candidate;

/// One candidate as the model sees it. A dedicated projection so adding a field
/// to [`Candidate`] can never silently enlarge the payload.
#[derive(serde::Serialize)]
pub struct PromptItem<'a> {
    pub id: String,
    pub path: &'a str,
    pub entry_type: &'static str,
    pub size_bytes: u64,
    pub age_days: u64,
    pub family: &'static str,
    pub signals: Vec<&'a str>,
}

/// Build the (system, user) message pair for one batch.
pub fn build(batch: &[Candidate], language: &str) -> (String, String) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    build_at(batch, language, now_ms)
}

/// [`build`] with an explicit clock, for reproducible prompts and tests.
pub fn build_at(batch: &[Candidate], language: &str, now_ms: i64) -> (String, String) {
    let system = format!(
        "You are a conservative disk-cleanup reviewer inside a desktop app. \
For each item you receive, decide whether it can be deleted.\n\
Reply with ONLY a JSON object containing a \"verdicts\" array. Each element: {{\
\"id\": string, \"safety\": \"safe\"|\"review\"|\"keep\", \"confidence\": number 0..1, \
\"reason\": string, \"impact\": string}}.\n\
Rules:\n\
- \"safe\" means deleting it loses nothing the user cannot restore or regenerate.\n\
- \"review\" means a human must decide. \"keep\" means it should not be deleted.\n\
- If you are unsure, answer \"review\". Never guess \"safe\".\n\
- \"reason\" is ONE short sentence in {language}, naming the object and why.\n\
- \"impact\" is ONE short sentence in {language} describing what breaks or is \
regenerated after deletion (which app must rebuild, which data is unrecoverable).\n\
- Do not propose shell commands: the app attaches the correct cleanup command.\n\
- Judge only what you are given. You cannot see file contents; do not imply that you did.\n\
- Return one element per item, using the exact ids given.",
        language = language
    );

    let items: Vec<PromptItem<'_>> = batch
        .iter()
        .map(|candidate| PromptItem {
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
    use std::path::PathBuf;

    fn candidate(display: &str) -> Candidate {
        Candidate::new(
            NodeKey::from_bytes(display.as_bytes()),
            PathBuf::from("/secret/home/x"),
            display,
            "x",
            true,
            ByteSize::new(1024, 4096),
            0,
            CandidateKind::StaleLargeFile,
        )
    }

    #[test]
    fn prompt_describes_contract_and_asks_for_impact() {
        let (system, _user) = build_at(&[candidate("~/x")], "zh", 0);
        assert!(system.contains("\"impact\""));
        assert!(system.contains("verdicts"));
        assert!(system.contains("Do not propose shell commands"));
    }

    #[test]
    fn payload_uses_redacted_path_only() {
        let (_system, user) = build_at(&[candidate("~/Downloads/a.dmg")], "zh", 0);
        assert!(user.contains("~/Downloads/a.dmg"));
        assert!(!user.contains("/secret/home"), "absolute path must not leak");
    }
}
