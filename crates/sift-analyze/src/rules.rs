//! Local rules that nominate cleanup candidates.
//!
//! This module is the gatekeeper: nothing reaches a language model or an
//! unattended cleanup unless a rule here nominated it first. That ordering is
//! deliberate — the rules are deterministic, auditable and free, and they
//! decide what a remote service is even allowed to learn about.
//!
//! Two conventions keep the rules trustworthy:
//!
//! * **Exact, case-sensitive names.** `node_modules` is a convention with one
//!   spelling; a folder called `Build` is far more likely to be the user's work
//!   than a stale artifact. Missing a candidate is cheap; nominating the wrong
//!   directory is not.
//! * **The safety of a pattern is decided here, not by a model.** A model may
//!   confirm or reject what a rule found; it may not invent a `Safe`.

use std::path::Path;

use crate::candidate::{CandidateKind, Evidence};
use crate::reason::Safety;

/// Thresholds a rule needs to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleThresholds {
    /// A file must be at least this large to be a stale-file candidate.
    pub stale_min_bytes: u64,
    /// A file must be at least this large to take part in duplicate detection.
    pub duplicate_min_bytes: u64,
    /// Days without modification before a large file counts as stale.
    pub stale_days: u64,
    /// A log file must be at least this large to be worth nominating.
    pub log_min_bytes: u64,
}

impl Default for RuleThresholds {
    fn default() -> Self {
        Self {
            stale_min_bytes: 256 * 1024 * 1024,
            duplicate_min_bytes: 8 * 1024 * 1024,
            stale_days: 180,
            log_min_bytes: 64 * 1024 * 1024,
        }
    }
}

/// A rule's conclusion about a path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleHit {
    /// Stable rule id, e.g. `dir.node_modules`. Recorded with the verdict so a
    /// conclusion can always be traced back to the rule that produced it.
    pub rule: &'static str,
    pub kind: CandidateKind,
    pub safety: Safety,
    /// i18n key for the reason sentence.
    pub reason_key: &'static str,
    /// Optional parameter the reason key interpolates.
    pub reason_param: Option<String>,
    /// i18n key for the supporting evidence line.
    pub detail_key: &'static str,
}

impl RuleHit {
    /// The candidate-facing conclusion for this hit.
    pub fn nomination(&self) -> crate::candidate::Nomination {
        crate::candidate::Nomination {
            rule: self.rule.to_string(),
            safety: self.safety,
            reason_key: self.reason_key.to_string(),
            reason_param: self.reason_param.clone(),
            detail_key: self.detail_key.to_string(),
        }
    }

    /// The evidence record for this hit.
    pub fn evidence(&self) -> Evidence {
        let evidence = Evidence::new(self.rule, self.detail_key);
        match &self.reason_param {
            Some(param) => evidence.with_param(param.clone()),
            None => evidence,
        }
    }
}

/// A directory-name rule.
struct DirRule {
    rule: &'static str,
    names: &'static [&'static str],
    /// `Rebuildable`/`Cache`/`Log` carry a tool or app name in the reason.
    outcome: DirOutcome,
    safety: Safety,
}

#[derive(Clone, Copy)]
enum DirOutcome {
    Rebuildable(&'static str),
    Cache(&'static str),
    Log(&'static str),
    Trash,
    /// Build output whose safety depends on the project, so it stays `Review`.
    BuildOutput(&'static str),
}

/// The directory rules. Order does not matter: names are unique across rules.
const DIR_RULES: &[DirRule] = &[
    DirRule {
        rule: "dir.node_modules",
        names: &["node_modules"],
        outcome: DirOutcome::Rebuildable("npm"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.cargo_target",
        names: &["target"],
        outcome: DirOutcome::Rebuildable("cargo"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.xcode_derived_data",
        names: &["DerivedData"],
        outcome: DirOutcome::Rebuildable("Xcode"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.python_pycache",
        names: &["__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache"],
        outcome: DirOutcome::Rebuildable("Python"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.gradle",
        names: &[".gradle"],
        outcome: DirOutcome::Rebuildable("Gradle"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.rust_gradle_build",
        names: &["build"],
        outcome: DirOutcome::BuildOutput("build"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.js_build_output",
        names: &["dist", ".next", ".nuxt", ".turbo", ".parcel-cache"],
        outcome: DirOutcome::BuildOutput("JS"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.python_venv",
        names: &[".venv"],
        outcome: DirOutcome::Rebuildable("Python venv"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.python_venv_named",
        names: &["venv"],
        outcome: DirOutcome::Rebuildable("Python venv"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.mobile_deps",
        names: &["Pods", "Carthage", "SourcePackages"],
        outcome: DirOutcome::Rebuildable("mobile deps"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.terraform",
        names: &[".terraform"],
        outcome: DirOutcome::Rebuildable("Terraform"),
        safety: Safety::Review,
    },
    DirRule {
        rule: "dir.app_cache",
        names: &["Caches"],
        outcome: DirOutcome::Cache("application"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.app_logs",
        names: &["Logs"],
        outcome: DirOutcome::Log("application"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.crash_reports",
        names: &["DiagnosticReports", "CrashReporter"],
        outcome: DirOutcome::Log("crash"),
        safety: Safety::Safe,
    },
    DirRule {
        rule: "dir.trash",
        names: &[".Trash", "Trash", ".Trash-1000"],
        outcome: DirOutcome::Trash,
        safety: Safety::Safe,
    },
];

/// Installer extensions. These are `Review`, not `Safe`: an installer is
/// usually disposable but occasionally the only copy of something.
const INSTALLER_EXTENSIONS: &[&str] =
    &["dmg", "pkg", "iso", "msi", "exe", "appimage", "deb", "rpm"];

/// Archive extensions, also `Review`.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "xz", "bz2", "7z", "rar", "tar.gz", "tar.xz", "tar.bz2", "tar.zst",
];

/// Log-ish extensions.
const LOG_EXTENSIONS: &[&str] = &["log", "crash", "ips", "dmp", "diag"];

/// Names that are pure noise wherever they appear.
const NOISE_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", ".localized"];

/// Ask the directory rules about `name`.
///
/// `parent` is used only for the two rules whose meaning depends on where they
/// sit (`Caches`/`Logs` inside a library directory, `Trash` inside a home).
pub fn for_dir(name: &str, path: &Path) -> Option<RuleHit> {
    let parent_name = path.parent().and_then(|parent| parent.file_name());

    for rule in DIR_RULES {
        if !rule.names.contains(&name) {
            continue;
        }
        // `Caches`/`Logs`/`Trash` are only those things inside a library or
        // home directory; a project folder called "Logs" is the user's.
        if matches!(rule.outcome, DirOutcome::Cache(_) | DirOutcome::Log(_)) {
            let in_library = path
                .components()
                .any(|component| component.as_os_str() == "Library");
            if !in_library {
                continue;
            }
        }
        if matches!(rule.outcome, DirOutcome::Trash) {
            let looks_like_trash_parent = parent_name
                .map(|parent| {
                    parent.to_string_lossy() == "share" || parent.to_string_lossy() == "."
                })
                .unwrap_or(false)
                || path
                    .components()
                    .any(|component| component.as_os_str() == "Trash");
            let in_home = path
                .parent()
                .map(|parent| !parent.to_string_lossy().is_empty())
                .unwrap_or(false);
            if !(looks_like_trash_parent || in_home) {
                continue;
            }
        }

        let (kind, reason_param) = match rule.outcome {
            DirOutcome::Rebuildable(tool) => (
                CandidateKind::RebuildableCache {
                    tool: tool.to_string(),
                },
                Some(tool.to_string()),
            ),
            DirOutcome::BuildOutput(tool) => (
                CandidateKind::RebuildableCache {
                    tool: tool.to_string(),
                },
                Some(tool.to_string()),
            ),
            DirOutcome::Cache(app) => (
                CandidateKind::CacheDirectory {
                    app: app.to_string(),
                },
                Some(app.to_string()),
            ),
            DirOutcome::Log(family) => (
                CandidateKind::Log {
                    family: family.to_string(),
                },
                Some(family.to_string()),
            ),
            DirOutcome::Trash => (CandidateKind::Trash, None),
        };

        let reason_key = match rule.outcome {
            DirOutcome::Rebuildable(_) | DirOutcome::BuildOutput(_) => "reason.rebuildableCache",
            DirOutcome::Cache(_) => "reason.cacheDirectory",
            DirOutcome::Log(_) => "reason.log",
            DirOutcome::Trash => "reason.trash",
        };

        // A regenerable directory that lives *inside* an installed bundle is not
        // regenerable: the user has no toolchain for a shipped application, and
        // removing it breaks the app rather than freeing space they can reclaim.
        // The same name in a project directory is a cache; here it is a repair.
        let inside_bundle = sift_core::deletable::inside_bundle(path);
        let downgraded = inside_bundle
            && matches!(
                rule.outcome,
                DirOutcome::Rebuildable(_) | DirOutcome::BuildOutput(_) | DirOutcome::Cache(_)
            );
        if downgraded {
            return Some(RuleHit {
                rule: rule.rule,
                kind,
                safety: Safety::Review,
                reason_key: "reason.insideBundle",
                reason_param,
                detail_key: "detail.insideBundle",
            });
        }

        return Some(RuleHit {
            rule: rule.rule,
            kind,
            safety: rule.safety,
            reason_key,
            reason_param,
            detail_key: "detail.patternMatch",
        });
    }
    None
}

/// Ask the file rules about a file.
///
/// `now_ms` is passed in rather than read from the clock so staleness rules are
/// deterministic in tests.
pub fn for_file(
    name: &str,
    path: &Path,
    size: u64,
    mtime_ms: i64,
    now_ms: i64,
    thresholds: &RuleThresholds,
) -> Option<RuleHit> {
    if size == 0 {
        // An empty file is never worth a user decision.
        return None;
    }

    if NOISE_NAMES.contains(&name) {
        return Some(RuleHit {
            rule: "file.os_noise",
            kind: CandidateKind::TempFile,
            safety: Safety::Safe,
            reason_key: "reason.tempFile",
            reason_param: None,
            detail_key: "detail.osGenerated",
        });
    }

    let extension = extension_of(name);

    if let Some(extension) = extension.as_deref() {
        if INSTALLER_EXTENSIONS.contains(&extension) {
            return Some(RuleHit {
                rule: "file.installer",
                kind: CandidateKind::PackageInstaller {
                    extension: extension.to_string(),
                },
                safety: Safety::Review,
                reason_key: "reason.packageInstaller",
                reason_param: Some(extension.to_string()),
                detail_key: "detail.installerExtension",
            });
        }
        if ARCHIVE_EXTENSIONS.contains(&extension) {
            return Some(RuleHit {
                rule: "file.archive",
                kind: CandidateKind::Archive {
                    extension: extension.to_string(),
                },
                safety: Safety::Review,
                reason_key: "reason.archive",
                reason_param: Some(extension.to_string()),
                detail_key: "detail.archiveExtension",
            });
        }
        if LOG_EXTENSIONS.contains(&extension) && size >= thresholds.log_min_bytes {
            let family = if extension == "log" {
                "application"
            } else {
                "crash"
            };
            return Some(RuleHit {
                rule: "file.large_log",
                kind: CandidateKind::Log {
                    family: family.to_string(),
                },
                safety: Safety::Safe,
                reason_key: "reason.log",
                reason_param: Some(family.to_string()),
                detail_key: "detail.largeLog",
            });
        }
    }

    // A large file nothing has touched in a long time. The age check uses the
    // file's own mtime only: directory mtimes are not transitive, so an old file
    // in an active directory is exactly what this rule is looking for.
    let age_days = age_days(mtime_ms, now_ms);
    if size >= thresholds.stale_min_bytes && age_days >= thresholds.stale_days {
        let _ = path;
        return Some(RuleHit {
            rule: "file.stale_large",
            kind: CandidateKind::StaleLargeFile,
            safety: Safety::Review,
            reason_key: "reason.staleLargeFile",
            reason_param: Some(age_days.to_string()),
            detail_key: "detail.untouched",
        });
    }

    None
}

/// Whole days between `mtime_ms` and `now_ms`, saturating at zero for files
/// with a future or unreadable timestamp.
pub fn age_days(mtime_ms: i64, now_ms: i64) -> u64 {
    if mtime_ms <= 0 || now_ms <= mtime_ms {
        return 0;
    }
    ((now_ms - mtime_ms) / 86_400_000) as u64
}

/// The lowercase extension of `name`, handling the two-part archive suffixes
/// (`tar.gz`, `tar.xz`) that matter for classification.
pub fn extension_of(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    for compound in ["tar.gz", "tar.xz", "tar.bz2", "tar.zst"] {
        if lower.ends_with(&format!(".{compound}")) {
            return Some(compound.to_string());
        }
    }
    let dot = lower.rfind('.')?;
    if dot == 0 || dot + 1 >= lower.len() {
        // A leading dot is a hidden file, not an extension.
        return None;
    }
    Some(lower[dot + 1..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DAY: i64 = 86_400_000;

    fn dir(name: &str) -> PathBuf {
        PathBuf::from("/Users/me/project").join(name)
    }

    #[test]
    fn node_modules_is_safe_and_regenerable() {
        let hit = for_dir("node_modules", &dir("node_modules")).expect("rule");
        assert_eq!(hit.safety, Safety::Safe);
        assert!(matches!(
            hit.kind,
            CandidateKind::RebuildableCache { ref tool } if tool == "npm"
        ));
        assert_eq!(hit.reason_key, "reason.rebuildableCache");
    }

    #[test]
    fn generic_build_names_stay_review() {
        // "build", "dist" and "target" are common project directories; a rule
        // may nominate them but must not call them safe.
        for name in ["build", "dist", "target", "venv", "Pods"] {
            let hit = for_dir(name, &dir(name)).expect(name);
            assert_eq!(hit.safety, Safety::Review, "{name} must stay Review");
        }
    }

    #[test]
    fn tool_conventions_are_case_sensitive() {
        // A user's "Build" folder is not a build artifact.
        assert!(for_dir("Build", &dir("Build")).is_none());
        assert!(for_dir("NODE_MODULES", &dir("NODE_MODULES")).is_none());
    }

    #[test]
    fn caches_and_logs_only_count_inside_a_library() {
        assert!(for_dir("Caches", Path::new("/Users/me/Library/Caches")).is_some());
        assert!(for_dir("Caches", Path::new("/Users/me/project/Caches")).is_none());
        assert!(for_dir("Logs", Path::new("/Users/me/Library/Logs")).is_some());
        assert!(for_dir("Logs", Path::new("/Users/me/project/Logs")).is_none());
    }

    /// The same directory name means different things inside and outside an
    /// application bundle, and only one of them is safe.
    #[test]
    fn a_cache_inside_an_app_bundle_is_not_safe() {
        let project = for_dir("node_modules", Path::new("/Users/me/project/node_modules"))
            .expect("project cache");
        assert_eq!(project.safety, Safety::Safe);

        let bundled = for_dir(
            "node_modules",
            Path::new("/Applications/ChatGPT.app/Contents/Resources/node_modules"),
        )
        .expect("bundled cache");
        assert_eq!(
            bundled.safety,
            Safety::Review,
            "an installed app's internals are not the user's to regenerate"
        );
        assert_eq!(bundled.reason_key, "reason.insideBundle");
        // The family is still reported, so the row can say what it is.
        assert!(matches!(
            bundled.kind,
            CandidateKind::RebuildableCache { .. }
        ));
    }

    #[test]
    fn an_app_cache_directory_inside_an_app_is_also_downgraded() {
        let hit = for_dir(
            "Caches",
            Path::new("/Applications/Notes.app/Contents/Library/Caches"),
        )
        .expect("bundled cache");
        assert_eq!(hit.safety, Safety::Review);
        assert_eq!(hit.reason_key, "reason.insideBundle");
    }

    #[test]
    fn trash_is_safe() {
        let hit = for_dir(".Trash", Path::new("/Users/me/.Trash")).expect("trash rule");
        assert_eq!(hit.safety, Safety::Safe);
        assert_eq!(hit.kind, CandidateKind::Trash);
    }

    #[test]
    fn installers_and_archives_are_review() {
        let dmg = for_file(
            "Installer.dmg",
            Path::new("/Users/me/Downloads/Installer.dmg"),
            1 << 30,
            DAY * 400,
            DAY * 400,
            &RuleThresholds::default(),
        )
        .expect("installer");
        assert_eq!(dmg.safety, Safety::Review);
        assert!(matches!(
            dmg.kind,
            CandidateKind::PackageInstaller { ref extension } if extension == "dmg"
        ));

        for (name, expected) in [("backup.tar.gz", "tar.gz"), ("photos.zip", "zip")] {
            let hit = for_file(
                name,
                Path::new("/Users/me").join(name).as_path(),
                1 << 30,
                0,
                0,
                &RuleThresholds::default(),
            )
            .unwrap_or_else(|| panic!("{name} should be an archive"));
            assert!(
                matches!(&hit.kind, CandidateKind::Archive { extension } if extension == expected),
                "{name} -> {:?}",
                hit.kind
            );
        }
    }

    #[test]
    fn stale_large_requires_both_size_and_age() {
        let thresholds = RuleThresholds::default();
        let big = 512 * 1024 * 1024;
        let old = DAY * 400;
        // Big and old: candidate.
        assert!(for_file(
            "video.mov",
            Path::new("/x/video.mov"),
            big,
            DAY,
            old,
            &thresholds
        )
        .is_some());
        // Big but fresh: not a candidate.
        assert!(for_file(
            "video.mov",
            Path::new("/x/video.mov"),
            big,
            old,
            old,
            &thresholds
        )
        .is_none());
        // Old but small: not a candidate.
        assert!(for_file(
            "notes.txt",
            Path::new("/x/notes.txt"),
            1024,
            DAY,
            old,
            &thresholds
        )
        .is_none());
    }

    #[test]
    fn empty_files_are_never_candidates() {
        assert!(for_file(
            "empty.dmg",
            Path::new("/x/empty.dmg"),
            0,
            0,
            0,
            &RuleThresholds::default()
        )
        .is_none());
    }

    #[test]
    fn os_noise_is_safe() {
        let hit = for_file(
            ".DS_Store",
            Path::new("/Users/me/.DS_Store"),
            6148,
            0,
            0,
            &RuleThresholds::default(),
        )
        .expect("noise");
        assert_eq!(hit.safety, Safety::Safe);
        assert_eq!(hit.kind, CandidateKind::TempFile);
    }

    #[test]
    fn large_logs_are_safe_but_small_ones_are_ignored() {
        let thresholds = RuleThresholds::default();
        let big = thresholds.log_min_bytes + 1;
        assert!(for_file("app.log", Path::new("/x/app.log"), big, 0, 0, &thresholds).is_some());
        assert!(for_file("app.log", Path::new("/x/app.log"), 1024, 0, 0, &thresholds).is_none());
    }

    #[test]
    fn age_days_handles_future_and_missing_timestamps() {
        assert_eq!(age_days(0, DAY * 100), 0);
        assert_eq!(age_days(DAY * 200, DAY * 100), 0);
        assert_eq!(age_days(DAY * 100, DAY * 130), 30);
    }

    #[test]
    fn compound_extensions_are_recognised() {
        assert_eq!(extension_of("a.tar.gz").as_deref(), Some("tar.gz"));
        assert_eq!(extension_of("a.TAR.GZ").as_deref(), Some("tar.gz"));
        assert_eq!(extension_of("a.ZIP").as_deref(), Some("zip"));
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("noext"), None);
    }

    #[test]
    fn every_dir_rule_is_reachable_and_distinct() {
        // Guards against a rule being shadowed by an earlier duplicate name.
        let mut seen: Vec<&str> = Vec::new();
        for rule in DIR_RULES {
            for name in rule.names {
                assert!(
                    !seen.contains(name),
                    "directory name {name} is claimed by two rules"
                );
                seen.push(name);
            }
            assert!(!rule.rule.is_empty());
        }
    }
}
