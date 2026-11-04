//! Explain what a directory is for, using the configured model.
//!
//! Only the path and a bounded sample of file *names* are sent — never file
//! contents. The answer is advisory text for the Explorer panel.

use std::sync::Arc;

use tauri::State;

use crate::analyze::AppServices;

/// How many file names the prompt may include.
const MAX_NAMES: usize = 40;

const SYSTEM: &str = r#"You explain what a directory on the user's computer is used for.

Base your answer only on the directory path and the file names provided. Never invent file contents. If the directory is unfamiliar, say what can be inferred and mark the rest as uncertain.

Answer in the same language as the user's request, in at most 120 words, as two short paragraphs:
1. what the directory is for;
2. whether its contents are usually safe to clean, and why."#;

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

#[tauri::command]
pub async fn ai_explain_dir(
    services: State<'_, AppServices>,
    path: String,
    names: Vec<String>,
) -> Result<String, String> {
    let store = Arc::clone(&services.store);
    tauri::async_runtime::spawn_blocking(move || explain(&store, path, names))
        .await
        .map_err(|error| format!("err.aiRequest|{error}"))?
}

fn explain(store: &sift_store::Store, path: String, names: Vec<String>) -> Result<String, String> {
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
            "Directory: {path}\nSample files (names only, not contents):\n{}\n\nWhat is this directory for?",
            sample.join("\n")
        );
        client
            .complete(SYSTEM, &user)
            .map_err(|error| map_error(&error.to_string(), &endpoint))
    }
    #[cfg(not(feature = "remote-ai"))]
    {
        let _ = (&profile, &endpoint);
        Err("err.aiNotConfigured".into())
    }
}
