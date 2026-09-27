//! Shared shape for cleanup results.
//!
//! The actual deletion command is `analyze::clean_paths`, the single path
//! through which the guardrails run. This module only defines the per-item
//! result type so the adapter layer needs no serialization duplication.

use serde::Serialize;

/// Per-item outcome shared by every cleanup command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResultItem {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_items_keep_their_shape() {
        let item = DeleteResultItem {
            path: "/tmp/x".into(),
            ok: false,
            error: Some("locked".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"path\""));
        assert!(json.contains("\"ok\":false"));
    }
}
