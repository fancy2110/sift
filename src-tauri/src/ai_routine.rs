//! Turn a sentence into a routine draft with the configured model.
//!
//! The model only ever produces *configuration*, never an action: it cannot
//! run commands, rename files or delete anything. The user's home directory is
//! substituted into the prompt so it can name absolute paths (no file contents
//! or other paths are sent). Paths are re-validated here
//! (absolute, existing directory, inside the home directory), the draft is
//! shown to the user, and saving always starts in approve mode. Nothing is
//! deleted from a model answer.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::analyze::AppServices;

/// Upper bound on paths in one generated routine.
const MAX_AI_PATHS: usize = 5;
/// Upper bound on the prompt length.
const MAX_PROMPT_CHARS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRoutineDraft {
    pub title: String,
    /// `cadence.daily` | `cadence.weekly` | `cadence.monthly`.
    pub cadence: String,
    pub paths: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
struct ModelReply {
    title: String,
    #[serde(default)]
    cadence: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    reason: String,
}

/// Build the system prompt with the real home directory substituted, so the
/// model returns absolute paths instead of guessing `~/` or `/Users/me`.
fn system_prompt(home: &Path) -> String {
    let home = home.display();
    format!(
        r#"You convert one user request into one Sift routine configuration.

You only return configuration. You never execute commands, rename files, or modify anything.

Output ONLY a single JSON object. No markdown fences, no explanation, no text before or after it, and no extra keys.

Required keys:
- "title": string, at most 40 characters.
- "cadence": exactly one of "daily", "weekly", "monthly".
- "paths": array of 1 to 5 absolute directory paths that already exist under {home}. Prefer caches, logs, temporary files and build output. Never use "/", "{home}" itself, or system paths.
- "reason": string, at most 120 characters, explaining why this is safe.

Write "title" and "reason" in the same language as the user's request.

The exact shape (values are placeholders):
{{"title":"...","cadence":"weekly","paths":["{home}/Library/..."],"reason":"..."}}"#
    )
}

/// Extract the first JSON object from a model reply (which may add prose or
/// code fences despite the instruction).
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&text[start..=end])
}

fn normalize_cadence(raw: &str) -> String {
    match raw
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("routine.")
        .trim_start_matches("cadence.")
    {
        "daily" => "routine.cadence.daily",
        "monthly" => "routine.cadence.monthly",
        _ => "routine.cadence.weekly",
    }
    .to_string()
}

/// Keep only absolute, existing directories inside the home directory.
/// Expand a leading `~` so the model can name the home directory without
/// knowing the account name.
fn expand_tilde(raw: &str) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    if raw == "~" {
        return Some(home);
    }
    match raw.strip_prefix("~/") {
        Some(rest) => Some(home.join(rest)),
        None => Some(std::path::PathBuf::from(raw)),
    }
}

fn validated_paths(paths: &[String]) -> Vec<String> {
    let home = dirs::home_dir();
    let mut kept: Vec<String> = Vec::new();
    for raw in paths {
        let Some(candidate) = expand_tilde(raw.trim()) else {
            continue;
        };
        if !candidate.is_absolute() {
            continue;
        }
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.is_dir() {
            continue;
        }
        if let Some(home) = &home {
            // The home directory itself is not a cleanup target.
            if canonical == *home || !canonical.starts_with(home) {
                continue;
            }
        }
        let value = canonical.to_string_lossy().to_string();
        if !kept.contains(&value) {
            kept.push(value);
        }
        if kept.len() >= MAX_AI_PATHS {
            break;
        }
    }
    kept
}

fn parse_draft(text: &str, prompt: &str) -> Result<AiRoutineDraft, String> {
    let json = extract_json(text).ok_or_else(|| "err.aiRoutineParse".to_string())?;
    let reply: ModelReply =
        serde_json::from_str(json).map_err(|_| "err.aiRoutineParse".to_string())?;
    let paths = validated_paths(&reply.paths);
    if paths.is_empty() {
        return Err("err.aiRoutineNoPaths".to_string());
    }
    let title = if reply.title.trim().is_empty() {
        prompt.chars().take(40).collect::<String>()
    } else {
        reply.title.trim().to_string()
    };
    Ok(AiRoutineDraft {
        title,
        cadence: normalize_cadence(&reply.cadence),
        paths,
        reason: reply.reason.trim().to_string(),
    })
}

/// Map a client error to the stable keys the UI already translates.
fn map_client_error(error: &str, endpoint: &str) -> String {
    if error.contains("[timeout]") || error.contains("timed out") {
        "err.aiTimeout".to_string()
    } else if error.contains("HTTP 404") {
        format!("err.aiNotFound|{endpoint}")
    } else if error.contains("HTTP 401") || error.contains("HTTP 403") {
        format!("err.aiAuthKeychain|{endpoint}")
    } else {
        format!("err.aiRequest|{error}")
    }
}

#[tauri::command]
pub async fn ai_routine(
    services: State<'_, AppServices>,
    prompt: String,
) -> Result<AiRoutineDraft, String> {
    let store = Arc::clone(&services.store);
    tauri::async_runtime::spawn_blocking(move || ai_routine_blocking(&store, prompt))
        .await
        .map_err(|error| format!("err.aiRequest|{error}"))?
}

fn ai_routine_blocking(
    store: &sift_store::Store,
    prompt: String,
) -> Result<AiRoutineDraft, String> {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("err.promptEmpty".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err("err.promptTooLong".into());
    }
    let ai = store.settings().ai;
    if !ai.is_usable() {
        return Err("err.aiNotConfigured".into());
    }
    let profile = ai
        .active()
        .cloned()
        .ok_or_else(|| "err.aiNotConfigured".to_string())?;
    let endpoint = crate::analyze::normalize_endpoint_for(&profile.endpoint, &profile.provider);

    #[cfg(feature = "remote-ai")]
    {
        let stored_key = match ai.api_key_from_env() {
            Some(key) => Some(key),
            None => {
                let account = crate::credentials::account_for(&profile.id);
                let mut result = crate::credentials::token_result(&account);
                if matches!(result, Ok(None)) && profile.id == "default" {
                    result =
                        crate::credentials::token_result(crate::credentials::legacy_account());
                }
                result.map_err(|error| format!("err.aiKeychain|{error}"))?
            }
        };
        let api_key = stored_key.ok_or_else(|| "err.aiTokenRequired".to_string())?;
        let config = sift_analyze::LlmConfig::new(
            profile.provider.clone(),
            endpoint.clone(),
            profile.model.clone(),
            api_key,
        )
        .with_consent()
        .with_language(ai.language.clone())
        .with_timeout(std::time::Duration::from_secs(120))
        .with_protocol(crate::analyze::protocol_for(&profile));
        let client = sift_analyze::OpenAiCompatibleAdjudicator::new(config)
            .map_err(|error| format!("err.aiRequest|{error}"))?;
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
        let system = system_prompt(&home);
        let reply = client
            .complete(&system, &prompt)
            .map_err(|error| map_client_error(&error.to_string(), &endpoint))?;
        parse_draft(&reply, &prompt)
    }
    #[cfg(not(feature = "remote-ai"))]
    {
        let _ = (&profile, &endpoint);
        Err("err.aiNotConfigured".into())
    }
}

/// Save a confirmed draft (manual or AI-generated). Always approve-before-run.
#[tauri::command]
pub fn save_routine_draft(
    services: State<'_, AppServices>,
    title: String,
    paths: Vec<String>,
    cadence: String,
) -> Result<(), String> {
    let paths = validated_paths(&paths);
    if paths.is_empty() {
        return Err("err.aiRoutineNoPaths".into());
    }
    let title = title.trim();
    if title.is_empty() {
        return Err("err.aiRoutineTitle".into());
    }
    let id = format!(
        "ai:{}",
        sift_core::NodeKey::from_path(Path::new(&paths[0]))
    );
    services.store.upsert_routine(sift_store::RoutineEntry {
        id,
        title: title.to_string(),
        kind: crate::analyze::CUSTOM_PATH_KIND.to_string(),
        cadence: normalize_cadence(&cadence),
        average_bytes: 0,
        mode: sift_store::RoutineMode::Approve,
        paths,
    });
    let _ = services.store.flush_if_dirty();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_fenced_reply_and_normalizes_the_cadence() {
        let reply = "```json\n{\"title\":\"清理构建缓存\",\"cadence\":\"daily\",\"paths\":[],\"reason\":\"x\"}\n```";
        // No valid paths -> the draft is rejected before it reaches the UI.
        assert!(parse_draft(reply, "prompt").is_err());

        let home = dirs::home_dir().unwrap();
        let inside = home.join("Library");
        let reply = format!(
            "{{\"title\":\"清理构建缓存\",\"cadence\":\"monthly\",\"paths\":[\"{}\"],\"reason\":\"r\"}}",
            inside.display()
        );
        let draft = parse_draft(&reply, "prompt").unwrap();
        assert_eq!(draft.cadence, "routine.cadence.monthly");
        assert_eq!(draft.title, "清理构建缓存");
    }

    #[test]
    fn tilde_paths_are_expanded_inside_home() {
        let kept = validated_paths(&["~/Library".to_string()]);
        let home = dirs::home_dir().unwrap();
        assert_eq!(kept.len(), 1);
        assert!(std::path::Path::new(&kept[0]).starts_with(&home));
        // A bare "~" is the home directory itself and must not be accepted.
        assert!(validated_paths(&["~".to_string()]).is_empty());
    }

    /// Live check: needs `ARK_KEY` in the environment and network access.
    /// Run with `cargo test -p sift --lib live_ai_routine -- --ignored`.
    #[test]
    #[ignore]
    fn live_ai_routine_returns_a_usable_draft() {
        if std::env::var("ARK_KEY").is_err() {
            return;
        }
        let store = sift_store::Store::in_memory();
        store.update_settings(|settings| {
            settings.ai.enabled = true;
            settings.ai.consent_granted = true;
            settings.ai.api_key_env = "ARK_KEY".to_string();
            settings.ai.profiles = vec![sift_store::AiProfile {
                id: "live".to_string(),
                name: "ark".to_string(),
                provider: "ark".to_string(),
                endpoint:
                    "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions".to_string(),
                model: "ark-code-latest".to_string(),
                api_key_env: String::new(),
            }];
            settings.ai.active_profile = "live".to_string();
        });
        let draft = ai_routine_blocking(&store, "每周清理 Xcode 构建缓存".to_string())
            .expect("live draft");
        assert!(!draft.paths.is_empty(), "no validated paths: {draft:?}");
        assert!(draft.cadence.starts_with("routine.cadence."));
        eprintln!("live draft: {draft:?}");
    }

    #[test]
    fn the_prompt_injects_the_home_directory_and_forbids_actions() {
        let home = dirs::home_dir().unwrap();
        let prompt = system_prompt(&home);
        assert!(prompt.contains(&home.display().to_string()));
        assert!(prompt.contains("never execute commands"));
        assert!(prompt.contains("\"cadence\""));
    }

    #[test]
    fn rejects_a_reply_without_json() {
        assert!(parse_draft("sorry, I cannot", "prompt").is_err());
    }

    #[test]
    fn keeps_only_existing_absolute_directories_inside_home() {
        let home = dirs::home_dir().unwrap();
        let inside = home.join("Library").join("Caches");
        let paths = vec![
            inside.to_string_lossy().to_string(),
            "/etc".to_string(),
            "relative/path".to_string(),
            inside.to_string_lossy().to_string(),
        ];
        let kept = validated_paths(&paths);
        // /etc is outside the home directory and the duplicate is collapsed;
        // the cache directory is kept only if it exists on this machine.
        assert!(kept.len() <= 1);
        for path in &kept {
            assert!(std::path::Path::new(path).starts_with(&home));
        }
    }
}
