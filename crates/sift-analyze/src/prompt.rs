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
use sift_apps::{AppFact, PmFact, ProjectFact, ScanContext};

/// Family-specific triage guidance, embedded at compile time.
///
/// Kept as a separate markdown asset so the policy can be reviewed and diffed
/// like code, while the JSON contract above stays small and stable. It only
/// covers the families that are model-adjudicable; structural families never
/// reach a model.
const TRIAGE_GUIDE: &str = include_str!("../prompts/triage.md");

/// How the model should use the environment snapshot. Kept short: the context
/// is optional, and its absence must not change the verdict contract.
const CONTEXT_GUIDE: &str = "Environment context:\n\
- You may also receive an `environment` object listing installed applications, \n\
  package managers and projects, plus their versions and redacted locations.\n\
- Use it to explain why a candidate exists or which tool would rebuild it.\n\
- Never invent an app, version, project or path that is not present. If a field\n\
  is missing, treat it as unknown.\n\
- Do not output commands. Maintenance actions are a closed enum handled by the\n\
  application, not free-form shell.\n";

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
    build_at_with_context(batch, language, now_ms, None)
}

/// Build the prompt with an optional environment snapshot. The snapshot is
/// redacted by `sift-apps` before it reaches this function; this module only
/// truncates it to a bounded prompt view.
pub fn build_at_with_context(
    batch: &[Candidate],
    language: &str,
    now_ms: i64,
    context: Option<&ScanContext>,
) -> (String, String) {
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
- Return one element per item, using the exact ids given.\n\n\
{guide}\n\
{context_guide}",
        language = language,
        guide = TRIAGE_GUIDE,
        context_guide = CONTEXT_GUIDE
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

    let mut user = format!(
        "Items to review:\n{}",
        serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
    );
    if let Some(context) = context {
        if !context.is_empty() {
            user.push_str("\n\nEnvironment context:\n");
            user.push_str(&environment_view(context));
        }
    }
    (system, user)
}

/// A bounded, redacted projection of the scan context for the prompt.
fn environment_view(context: &ScanContext) -> String {
    #[derive(serde::Serialize)]
    struct View<'a> {
        apps: &'a [AppFact],
        package_managers: &'a [PmFact],
        projects: &'a [ProjectFact],
        truncated: bool,
    }
    const MAX_APPS: usize = 40;
    const MAX_PMS: usize = 20;
    const MAX_PROJECTS: usize = 20;
    let truncated = context.apps.len() > MAX_APPS
        || context.package_managers.len() > MAX_PMS
        || context.projects.len() > MAX_PROJECTS;
    let view = View {
        apps: &context.apps[..context.apps.len().min(MAX_APPS)],
        package_managers: &context.package_managers[..context.package_managers.len().min(MAX_PMS)],
        projects: &context.projects[..context.projects.len().min(MAX_PROJECTS)],
        truncated,
    };
    serde_json::to_string_pretty(&view).unwrap_or_else(|_| "{}".to_string())
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
    fn prompt_includes_family_specific_triage_guidance() {
        let (system, _user) = build_at(&[candidate("~/Downloads/x.dmg")], "zh", 0);
        for family in [
            "staleLargeFile",
            "archive",
            "packageInstaller",
            "duplicateGroup",
        ] {
            assert!(
                system.contains(family),
                "system prompt is missing {family} guidance"
            );
        }
        assert!(
            system.contains("Never answer `safe` for this family"),
            "duplicate groups must never be model-authorized safe"
        );
        assert!(
            system.contains("at least 0.85"),
            "the confident-safe threshold must be stated"
        );
        assert!(system.len() > 2_000, "triage guidance was not embedded");
    }

    #[test]
    fn environment_context_is_included_and_bounded() {
        let mut context = ScanContext::default();
        context.scanned_at_ms = 1;
        context.apps.push(AppFact {
            key: "com.example".into(),
            name: "Example".into(),
            version: Some("1.2".into()),
            bundle_id: Some("com.example".into()),
            install_path: "~/Applications/Example.app".into(),
            last_used_days: Some(3),
            artifact_count: 4,
        });
        context.package_managers.push(PmFact {
            id: "pnpm".into(),
            version: Some("9.12".into()),
            detected: true,
            roots: vec!["~/.local/share/pnpm/store".into()],
            projects: vec!["~/code/app".into()],
            capabilities: vec!["pruneStore".into()],
        });
        let (system, user) = build_at_with_context(
            &[candidate("~/Downloads/a.dmg")],
            "zh",
            0,
            Some(&context),
        );
        assert!(system.contains("Environment context"));
        assert!(user.contains("Environment context"));
        assert!(user.contains("com.example"));
        assert!(user.contains("pnpm"));
        assert!(!user.contains("/secret/home"));
    }

    #[test]
    fn empty_context_does_not_change_the_user_message() {
        let without = build_at(&[candidate("~/x")], "zh", 0);
        let with = build_at_with_context(
            &[candidate("~/x")],
            "zh",
            0,
            Some(&ScanContext::default()),
        );
        assert_eq!(without.1, with.1);
    }

    #[test]
    fn payload_uses_redacted_path_only() {
        let (_system, user) = build_at(&[candidate("~/Downloads/a.dmg")], "zh", 0);
        assert!(user.contains("~/Downloads/a.dmg"));
        assert!(!user.contains("/secret/home"), "absolute path must not leak");
    }
}
