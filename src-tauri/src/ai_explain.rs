//! Explain what a directory is for, using the configured model.
//!
//! Only the path and a bounded sample of file *names* are sent — never file
//! contents. The answer is advisory text for the Explorer panel.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::analyze::AppServices;

/// How many file names the prompt may include.
const MAX_NAMES: usize = 40;

const SYSTEM: &str = r#"You classify the children of one directory on the user's computer.

For every provided name, decide what it is for and whether it can be deleted. Base the answer only on the directory path and the names — never invent file contents. When unsure, use "caution".

Reply with JSON only, no markdown and no prose:
{"items":[{"name":"<exact name>","verdict":"safe|caution|keep","purpose":"<at most 18 words>"}]}

Verdicts:
- "safe": caches, build output, logs or temporary data that regenerate;
- "caution": likely removable but may lose recent state or require a re-download;
- "keep": user documents, app data that does not regenerate, or system content."#;

/// Human-readable name for a UI language tag, so the model answers in it.
fn language_name(tag: &str) -> &'static str {
    match tag {
        "zh" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "ru" => "Russian",
        _ => "English",
    }
}

fn map_error(error: &str, endpoint: &str) -> String {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirVerdictDto {
    pub name: String,
    /// `safe` | `caution` | `keep`.
    pub verdict: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirVerdictsDto {
    pub items: Vec<DirVerdictDto>,
}

#[tauri::command]
pub async fn ai_explain_dir(
    services: State<'_, AppServices>,
    path: String,
    names: Vec<String>,
    language: Option<String>,
) -> Result<DirVerdictsDto, String> {
    let store = Arc::clone(&services.store);
    let language = language.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || explain(&store, path, names, &language))
        .await
        .map_err(|error| format!("err.aiRequest|{error}"))?
}

fn explain(
    store: &sift_store::Store,
    path: String,
    names: Vec<String>,
    language: &str,
) -> Result<DirVerdictsDto, String> {
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

        let sample: Vec<String> = names
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .take(MAX_NAMES)
            .map(|name| name.to_string())
            .collect();
        let user = format!(
            "Directory: {path}\nChildren (names only, not contents):\n{}\n\nClassify every child. Write each purpose in {}.",
            sample.join("\n"),
            language_name(language)
        );
        let reply = client
            .complete(SYSTEM, &user)
            .map_err(|error| map_error(&error.to_string(), &endpoint))?;
        parse_verdicts(&reply)
    }
    #[cfg(not(feature = "remote-ai"))]
    {
        let _ = (&profile, &endpoint);
        Err("err.aiNotConfigured".into())
    }
}

/// Parse the model's JSON, tolerating a bare array or stray prose.
fn parse_verdicts(reply: &str) -> Result<DirVerdictsDto, String> {
    let start = reply.find('{').ok_or_else(|| "err.aiRoutineParse".to_string())?;
    let end = reply.rfind('}').ok_or_else(|| "err.aiRoutineParse".to_string())?;
    let parsed: DirVerdictsDto = serde_json::from_str(&reply[start..=end])
        .map_err(|_| "err.aiRoutineParse".to_string())?;
    let items = parsed
        .items
        .into_iter()
        .filter(|item| !item.name.trim().is_empty())
        .map(|item| {
            let verdict = match item.verdict.trim().to_ascii_lowercase().as_str() {
                "safe" => "safe",
                "keep" => "keep",
                _ => "caution",
            };
            DirVerdictDto {
                name: item.name.trim().to_string(),
                verdict: verdict.to_string(),
                purpose: item.purpose.trim().chars().take(160).collect(),
            }
        })
        .collect();
    Ok(DirVerdictsDto { items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verdicts_and_normalizes_unknown_values() {
        let reply = r#"Here: {"items":[{"name":"Library","verdict":"safe","purpose":"Caches"},{"name":"Photos","verdict":"KEEP","purpose":"Your library"},{"name":"x","verdict":"maybe","purpose":"?"}]}"#;
        let parsed = parse_verdicts(reply).unwrap();
        assert_eq!(parsed.items.len(), 3);
        assert_eq!(parsed.items[0].verdict, "safe");
        assert_eq!(parsed.items[1].verdict, "keep");
        assert_eq!(parsed.items[2].verdict, "caution");
    }

    #[test]
    fn rejects_a_reply_without_json() {
        assert!(parse_verdicts("no idea").is_err());
    }
}
