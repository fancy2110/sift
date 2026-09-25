//! What gets analyzed: cleanup candidates and the evidence that nominated them.
//!
//! A candidate is *nominated* by a local rule — never by a model. The model's
//! job is narrower: confirm or reject a nomination and explain it. That split
//! matters because nomination is the step that decides what a model is even
//! allowed to see, so it must be local, auditable, and cheap.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sift_core::{ByteSize, NodeKey};

use crate::reason::PathFingerprint;

/// The families of thing the product knows how to reason about.
///
/// Anything that does not fit one of these is not a candidate: the analyzer
/// gives no opinion about unknown content, and an unknown candidate is one an
/// unattended cleanup can never touch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CandidateKind {
    /// A dependency, build, or index directory a toolchain regenerates
    /// (`node_modules`, `target`, `DerivedData`, `__pycache__`, …).
    RebuildableCache { tool: String },
    /// An OS or application cache directory.
    CacheDirectory { app: String },
    /// An installer or disk image that has served its purpose.
    PackageInstaller { extension: String },
    /// A plain archive that is large enough to be worth deciding about.
    Archive { extension: String },
    /// A large file nothing has touched in a long time.
    StaleLargeFile,
    /// One member of a set of same-name, same-size files.
    DuplicateGroup { name: String, copies: u32 },
    /// Already in the trash: emptying it is the definition of safe.
    Trash,
    /// A log or diagnostic report directory/file.
    Log { family: String },
    /// A temporary file the OS would have removed on reboot anyway.
    TempFile,
    /// Something the user marked themselves.
    UserMarked,
}

impl CandidateKind {
    /// A short stable token used in rule ids, tests, and the store schema.
    pub fn token(&self) -> &'static str {
        match self {
            CandidateKind::RebuildableCache { .. } => "rebuildableCache",
            CandidateKind::CacheDirectory { .. } => "cacheDirectory",
            CandidateKind::PackageInstaller { .. } => "packageInstaller",
            CandidateKind::Archive { .. } => "archive",
            CandidateKind::StaleLargeFile => "staleLargeFile",
            CandidateKind::DuplicateGroup { .. } => "duplicateGroup",
            CandidateKind::Trash => "trash",
            CandidateKind::Log { .. } => "log",
            CandidateKind::TempFile => "tempFile",
            CandidateKind::UserMarked => "userMarked",
        }
    }

    /// i18n key naming the family in a list row.
    pub fn label_key(&self) -> &'static str {
        match self {
            CandidateKind::RebuildableCache { .. } => "candidate.rebuildableCache",
            CandidateKind::CacheDirectory { .. } => "candidate.cacheDirectory",
            CandidateKind::PackageInstaller { .. } => "candidate.packageInstaller",
            CandidateKind::Archive { .. } => "candidate.archive",
            CandidateKind::StaleLargeFile => "candidate.staleLargeFile",
            CandidateKind::DuplicateGroup { .. } => "candidate.duplicateGroup",
            CandidateKind::Trash => "candidate.trash",
            CandidateKind::Log { .. } => "candidate.log",
            CandidateKind::TempFile => "candidate.tempFile",
            CandidateKind::UserMarked => "candidate.userMarked",
        }
    }

    /// Whether a language model may even be asked about this family.
    ///
    /// Model judgment is reserved for content whose safety genuinely depends on
    /// knowing what it is — a stale file, an archive, a duplicate set. For
    /// families whose safety is a property of the *pattern* rather than the
    /// content (`node_modules`, trash), asking a model would add cost and
    /// variance without adding information, and it would give a
    /// non-deterministic actor authority over a deterministic conclusion.
    pub fn is_model_adjudicable(&self) -> bool {
        matches!(
            self,
            CandidateKind::StaleLargeFile
                | CandidateKind::Archive { .. }
                | CandidateKind::PackageInstaller { .. }
                | CandidateKind::DuplicateGroup { .. }
        )
    }

    /// Whether a model is allowed to conclude [`crate::Safety::Safe`] here.
    ///
    /// A model may confirm what a rule already found removable; it may never be
    /// the sole reason something becomes automatically removable. Build caches
    /// and trash are the exception: their safety is structural, so a model
    /// agreeing adds nothing and a model disagreeing changes nothing.
    pub fn model_may_authorize_safe(&self) -> bool {
        matches!(
            self,
            CandidateKind::StaleLargeFile
                | CandidateKind::Archive { .. }
                | CandidateKind::PackageInstaller { .. }
        )
    }
}

/// One reason a candidate was nominated, kept for explanation and debugging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Evidence {
    /// The rule id that fired, e.g. `dir.node_modules`.
    pub rule: String,
    /// i18n key for the supporting detail.
    pub detail_key: String,
    /// Optional parameter for the detail key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

impl Evidence {
    pub fn new(rule: impl Into<String>, detail_key: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            detail_key: detail_key.into(),
            param: None,
        }
    }

    pub fn with_param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }
}

/// The conclusion the nominating rule reached.
///
/// Carried on the candidate rather than re-derived later from the rule id. The
/// earlier design reconstructed safety from a rule-name table in the adjudicator,
/// which silently discarded any nuance a rule added — a cache inside an
/// application bundle is `Review`, not `Safe`, and the table threw that away.
/// One nomination, one source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nomination {
    /// The rule id that nominated this candidate.
    pub rule: String,
    pub safety: crate::reason::Safety,
    /// i18n key for the reason sentence.
    pub reason_key: String,
    /// Optional parameter the reason key interpolates.
    pub reason_param: Option<String>,
    /// i18n key for the supporting evidence line.
    pub detail_key: String,
}

/// A nominated cleanup candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Candidate {
    pub key: NodeKey,
    /// The real absolute path. Local-only: never serialized into a remote
    /// request, never written to a shared log.
    pub path: PathBuf,
    /// The path as it should be shown to a user or a model: the home directory
    /// is collapsed to `~`, and nothing outside the scanned root leaks.
    pub display_path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: ByteSize,
    pub mtime_ms: i64,
    pub kind: CandidateKind,
    pub evidence: Vec<Evidence>,
    /// What the nominating rule concluded. `None` means "nominated without a
    /// conclusion", which the adjudicator treats as `Review` — never `Safe`.
    pub nomination: Option<Nomination>,
}

impl Candidate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: NodeKey,
        path: PathBuf,
        display_path: impl Into<String>,
        name: impl Into<String>,
        is_dir: bool,
        size: ByteSize,
        mtime_ms: i64,
        kind: CandidateKind,
    ) -> Self {
        Self {
            key,
            path,
            display_path: display_path.into(),
            name: name.into(),
            is_dir,
            size,
            mtime_ms,
            kind,
            evidence: Vec::new(),
            nomination: None,
        }
    }

    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }

    /// Attach the nominating rule's conclusion and its evidence record.
    pub fn with_nomination(mut self, nomination: Nomination) -> Self {
        self.evidence.push(Evidence {
            rule: nomination.rule.clone(),
            detail_key: nomination.detail_key.clone(),
            param: nomination.reason_param.clone(),
        });
        self.nomination = Some(nomination);
        self
    }

    /// The bytes reclaiming this would return, preferring real allocation.
    pub fn reclaimable(&self) -> u64 {
        self.size.reclaimable()
    }

    /// What a stored verdict must match to still apply.
    pub fn fingerprint(&self) -> PathFingerprint {
        PathFingerprint::new(self.key, self.size.logical, self.mtime_ms, self.is_dir)
    }

    /// A one-line description for logs and model prompts.
    pub fn summary(&self) -> String {
        format!(
            "{} ({}, {})",
            self.display_path,
            if self.is_dir { "directory" } else { "file" },
            sift_core::format_bytes(self.size.dominant())
        )
    }
}

/// Collapse a path to something safe to show or transmit.
///
/// The home prefix becomes `~`; a path outside both the home and the scan root
/// keeps only its final component, so a remote adjudicator learns the shape of
/// the tree without learning the user's directory names.
pub fn redact_path(path: &Path, home: Option<&Path>, root: Option<&Path>) -> String {
    if let Some(home) = home {
        if let Ok(rest) = path.strip_prefix(home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    if let Some(root) = root {
        if let Ok(rest) = path.strip_prefix(root) {
            if rest.as_os_str().is_empty() {
                return root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.to_string_lossy().into_owned());
            }
            return format!(
                "{}/{}",
                root.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.to_string_lossy().into_owned()),
                rest.to_string_lossy()
            );
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuildable_caches_are_not_sent_to_a_model() {
        let node_modules = CandidateKind::RebuildableCache {
            tool: "npm".into(),
        };
        assert!(!node_modules.is_model_adjudicable());
        assert!(!node_modules.model_may_authorize_safe());
        let stale = CandidateKind::StaleLargeFile;
        assert!(stale.is_model_adjudicable());
        assert!(stale.model_may_authorize_safe());
    }

    #[test]
    fn redaction_collapses_home() {
        let home = Path::new("/Users/me");
        assert_eq!(
            redact_path(Path::new("/Users/me/Downloads/a.dmg"), Some(home), None),
            "~/Downloads/a.dmg"
        );
        assert_eq!(redact_path(home, Some(home), None), "~");
    }

    #[test]
    fn redaction_falls_back_to_the_scan_root() {
        let root = Path::new("/Volumes/Backup");
        assert_eq!(
            redact_path(Path::new("/Volumes/Backup/movies"), None, Some(root)),
            "Backup/movies"
        );
    }

    #[test]
    fn redaction_keeps_only_the_name_when_nothing_matches() {
        // A path outside home and root must not reveal the user's directory
        // names to a remote service.
        assert_eq!(
            redact_path(Path::new("/elsewhere/secret/project"), None, None),
            "project"
        );
    }

    #[test]
    fn fingerprint_tracks_size_and_kind() {
        let candidate = Candidate::new(
            NodeKey::from_bytes(b"/a/b"),
            PathBuf::from("/a/b"),
            "~/b",
            "b",
            true,
            ByteSize::new(4096, 8192),
            99,
            CandidateKind::Trash,
        );
        let fingerprint = candidate.fingerprint();
        assert_eq!(fingerprint.size, 4096);
        assert_eq!(fingerprint.mtime_ms, 99);
        assert!(fingerprint.is_dir);
        assert_eq!(candidate.reclaimable(), 8192);
        assert!(candidate.summary().contains("~/b"));
    }

    #[test]
    fn kind_tokens_are_stable_and_unique() {
        let kinds = [
            CandidateKind::RebuildableCache { tool: "x".into() },
            CandidateKind::CacheDirectory { app: "x".into() },
            CandidateKind::PackageInstaller {
                extension: "dmg".into(),
            },
            CandidateKind::Archive {
                extension: "zip".into(),
            },
            CandidateKind::StaleLargeFile,
            CandidateKind::DuplicateGroup {
                name: "x".into(),
                copies: 2,
            },
            CandidateKind::Trash,
            CandidateKind::Log {
                family: "x".into(),
            },
            CandidateKind::TempFile,
            CandidateKind::UserMarked,
        ];
        let mut tokens: Vec<&str> = kinds.iter().map(|kind| kind.token()).collect();
        tokens.sort_unstable();
        let before = tokens.len();
        tokens.dedup();
        assert_eq!(tokens.len(), before, "tokens must be unique");
    }
}
