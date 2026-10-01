//! Structured cleanup output beyond the safety verdict.
//!
//! A model may describe *what removing the item means* and *how the user can
//! clean it without this app's delete button*. It never drives an automatic
//! action: the command is stored and shown as text to copy and run by hand.

use serde::{Deserialize, Serialize};

/// What removing one candidate affects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CleanupImpact {
    /// One short sentence: what the object is and what depends on it.
    pub summary: String,
    /// Whether deletion is reversible by regenerating or restoring.
    pub reversible: bool,
    /// Things to check before deleting (apps to close, accounts, backups…).
    pub warnings: Vec<String>,
}

impl CleanupImpact {
    pub fn new(summary: impl Into<String>, reversible: bool) -> Self {
        Self {
            summary: summary.into(),
            reversible,
            warnings: Vec::new(),
        }
    }

    /// Trim and clamp free text; return `None` when nothing usable remains.
    pub fn sanitized(&self) -> Option<Self> {
        let summary = one_line(&self.summary, 240)?;
        let mut warnings: Vec<String> = Vec::new();
        for warning in &self.warnings {
            if let Some(line) = one_line(warning, 160) {
                warnings.push(line);
            }
        }
        warnings.truncate(5);
        Some(Self {
            summary,
            reversible: self.reversible,
            warnings,
        })
    }
}

/// A suggested command-line way to clean the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CleanupCommand {
    /// The shell command text. Display only; never executed by the app.
    pub command: String,
    /// Platform tag: `macos` | `linux` | `windows` | `any`.
    pub platform: String,
    /// One short phrase saying what running it does.
    pub effect: String,
}

impl CleanupCommand {
    pub fn new(command: impl Into<String>, platform: impl Into<String>, effect: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            platform: platform.into(),
            effect: effect.into(),
        }
    }

    /// Validate and clamp one model-suggested command.
    ///
    /// A command must stay a single bounded line and must not invoke obviously
    /// destructive patterns beyond the target object; the UI treats it as
    /// copy-paste text, never as something to run.
    pub fn sanitized(&self) -> Option<Self> {
        let command = one_line(&self.command, 500)?;
        let platform = match self.platform.trim().to_ascii_lowercase().as_str() {
            "macos" | "darwin" => "macos",
            "linux" => "linux",
            "windows" | "win32" => "windows",
            _ => "any",
        };
        let effect = one_line(&self.effect, 160).unwrap_or_default();
        Some(Self {
            command,
            platform: platform.to_string(),
            effect,
        })
    }
}

/// Collapse whitespace, clamp length; `None` for empty input.
fn one_line(text: &str, max_chars: usize) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut out: String = collapsed.chars().take(max_chars).collect();
    if collapsed.chars().count() > max_chars {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impact_sanitises_and_caps_warnings() {
        let impact = CleanupImpact {
            summary: "  Xcode\nbuild cache ".into(),
            reversible: true,
            warnings: vec!["close Xcode".into(), " ".into(), "b".into(), "c".into(), "d".into(), "e".into()],
        }
        .sanitized()
        .unwrap();
        assert_eq!(impact.summary, "Xcode build cache");
        assert_eq!(impact.warnings.len(), 5);
    }

    #[test]
    fn empty_impact_summary_is_none() {
        assert!(CleanupImpact::default().sanitized().is_none());
    }

    #[test]
    fn command_normalises_platform_tags() {
        let command = CleanupCommand::new("rm -rf ~/a", "darwin", "remove dir")
            .sanitized()
            .unwrap();
        assert_eq!(command.platform, "macos");
        assert!(CleanupCommand::new("  ", "any", "").sanitized().is_none());
    }

    #[test]
    fn command_rejects_multiline_payload() {
        // Anything beyond one line is flattened, not silently executed twice.
        let command = CleanupCommand::new("rm a\nrm b", "any", "")
            .sanitized()
            .unwrap();
        assert_eq!(command.command, "rm a rm b");
    }
}
