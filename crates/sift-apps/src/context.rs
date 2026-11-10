//! The environment snapshot that accompanies every scan.
//!
//! It is deliberately small: identity, version, redacted location and a few
//! counts. File contents and real home paths never enter this type, so it can
//! be logged, cached and (for model-adjudicable fields) serialized into a
//! prompt without a second privacy review.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bump when a field changes meaning, so cached verdicts keyed by a context
/// hash cannot be reused across incompatible contexts.
pub const SCHEMA_VERSION: u32 = 1;

/// Everything the analyzer needs to know about the machine's installed
/// environment for one scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanContext {
    pub schema_version: u32,
    pub scanned_at_ms: i64,
    /// Scan roots, redacted for display.
    pub roots: Vec<String>,
    pub apps: Vec<AppFact>,
    pub package_managers: Vec<PmFact>,
    pub projects: Vec<ProjectFact>,
    /// True when inventory failed or a cached snapshot was reused.
    pub stale: bool,
    /// True when a cap truncated the inventory.
    pub truncated: bool,
}

impl Default for ScanContext {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            scanned_at_ms: 0,
            roots: Vec::new(),
            apps: Vec::new(),
            package_managers: Vec::new(),
            projects: Vec::new(),
            stale: false,
            truncated: false,
        }
    }
}

impl ScanContext {
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.package_managers.is_empty() && self.projects.is_empty()
    }

    /// A stable hash of the *facts a verdict depends on*. The exact timestamp
    /// and root list are excluded so a rescan of the same environment reuses
    /// cached model conclusions.
    pub fn fingerprint(&self) -> String {
        #[derive(Serialize)]
        struct Facts<'a> {
            schema: u32,
            apps: &'a [AppFact],
            package_managers: &'a [PmFact],
            projects: &'a [ProjectFact],
        }
        let facts = Facts {
            schema: self.schema_version,
            apps: &self.apps,
            package_managers: &self.package_managers,
            projects: &self.projects,
        };
        let json = serde_json::to_string(&facts).unwrap_or_default();
        // A small, dependency-free FNV-1a keeps this cheap; the hash is only a
        // cache key, not a security boundary.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in json.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// One installed desktop application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFact {
    /// Stable identity: bundle id / package family / install path hash.
    pub key: String,
    pub name: String,
    pub version: Option<String>,
    pub bundle_id: Option<String>,
    /// Redacted for display (`~/Applications/Foo.app` or `/Applications/Foo.app`).
    pub install_path: String,
    pub last_used_days: Option<u32>,
    /// Number of known artifacts, once a profile exists.
    pub artifact_count: u32,
}

/// One detected package manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PmFact {
    pub id: String,
    pub version: Option<String>,
    pub detected: bool,
    /// Redacted roots (store/cache/config/bin).
    pub roots: Vec<String>,
    /// Project roots (redacted) that use this manager.
    pub projects: Vec<String>,
    /// Closed capability tokens, never commands.
    pub capabilities: Vec<String>,
}

/// One project detected under a scan root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFact {
    /// Redacted project root.
    pub root: String,
    pub pm: String,
    pub manifest: String,
    pub lockfile: Option<String>,
    pub pm_version: Option<String>,
    pub dirty: bool,
}

/// Replace a home prefix with `~` and leave other paths alone.
pub fn redact_path(path: &Path, home: Option<&Path>) -> String {
    let text = path.to_string_lossy();
    if let Some(home) = home {
        let home_text = home.to_string_lossy();
        if text == home_text {
            return "~".to_string();
        }
        let with_sep = format!(
            "{}{}",
            home_text.trim_end_matches(['/', '\\']),
            std::path::MAIN_SEPARATOR
        );
        if let Some(rest) = text.strip_prefix(&with_sep) {
            return format!("~{}{}", std::path::MAIN_SEPARATOR, rest);
        }
    }
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_hides_the_home_prefix_only() {
        // Build the fixture with the platform separator so the test is not
        // Unix-specific.
        let home = std::env::temp_dir().join("sift-home");
        let inside = home.join("Library").join("Caches").join("x");
        let redacted = redact_path(&inside, Some(&home));
        assert!(redacted.starts_with('~'));
        assert!(redacted.ends_with(&format!(
            "Library{0}Caches{0}x",
            std::path::MAIN_SEPARATOR
        )));
        assert_eq!(
            redact_path(Path::new("/Applications/Foo.app"), Some(&home)),
            "/Applications/Foo.app"
        );
        assert_eq!(redact_path(&home, Some(&home)), "~");
    }

    #[test]
    fn context_fingerprint_ignores_clock_and_roots() {
        let mut a = ScanContext::default();
        a.scanned_at_ms = 1;
        a.roots.push("~/code".into());
        let mut b = a.clone();
        b.scanned_at_ms = 2;
        b.roots = vec!["~/other".into()];
        assert_eq!(a.fingerprint(), b.fingerprint());

        b.apps.push(AppFact {
            key: "com.example".into(),
            name: "Example".into(),
            version: Some("1.0".into()),
            bundle_id: Some("com.example".into()),
            install_path: "/Applications/Example.app".into(),
            last_used_days: None,
            artifact_count: 0,
        });
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn context_round_trips_as_camel_case_json() {
        let context = ScanContext {
            scanned_at_ms: 7,
            roots: vec!["~/code".into()],
            ..ScanContext::default()
        };
        let json = serde_json::to_string(&context).unwrap();
        assert!(json.contains("\"scannedAtMs\""));
        let back: ScanContext = serde_json::from_str(&json).unwrap();
        assert_eq!(back, context);
    }
}
