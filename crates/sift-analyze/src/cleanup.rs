//! Cleanup knowledge base: *how* a candidate should be cleaned.
//!
//! Detection decides what is reclaimable; this module decides the safest way to
//! reclaim it. The important rule is that anything a toolchain can maintain is
//! cleaned with the toolchain's own command instead of a raw delete:
//!
//! * Rust build output is `cargo clean`, not `rm -rf target` — the command
//!   knows about workspaces, registries and incremental artifacts.
//! * Homebrew's download cache is `brew cleanup`, which keeps the formulae.
//! * The pnpm global store is `pnpm store prune`, which keeps linked packages.
//!
//! Every entry is a *recommendation*: Sift never runs a destructive command
//! itself. The command, its scope and the impact of running it are data the UI
//! shows and the user confirms.

use crate::candidate::CandidateKind;

/// How the cleanup is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CleanupMethod {
    /// A toolchain-native maintenance command is preferred.
    NativeCommand,
    /// Move the item to the system trash (recoverable).
    TrashItem,
    /// Empty a trash location (already discarded, not recoverable).
    EmptyTrash,
    /// Remove the contents of a cache directory but keep the folder.
    ClearContents,
}

/// One shell step, without a shell so it can be shown or executed precisely.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandStep {
    /// Executable, e.g. `cargo`.
    pub program: &'static str,
    /// Arguments, e.g. `["clean"]`.
    pub args: &'static [&'static str],
    /// i18n key describing the directory the command must run from, if any.
    pub run_from_key: Option<&'static str>,
    /// Whether the step only applies to a global/tool-managed location.
    pub global: bool,
}

impl CleanupMethod {
    /// Stable token used in DTOs and persistence.
    pub fn token(self) -> &'static str {
        match self {
            CleanupMethod::NativeCommand => "nativeCommand",
            CleanupMethod::TrashItem => "trashItem",
            CleanupMethod::EmptyTrash => "emptyTrash",
            CleanupMethod::ClearContents => "clearContents",
        }
    }
}

/// A complete, explainable cleanup plan for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanupPlan {
    pub method: CleanupMethod,
    /// Preferred toolchain command, when one exists.
    pub command: Option<CommandStep>,
    /// i18n key for what happens after cleanup (the impact).
    pub impact_key: &'static str,
    /// i18n key for a short "how it is cleaned" description.
    pub method_key: &'static str,
}

impl CleanupPlan {
    fn native(
        program: &'static str,
        args: &'static [&'static str],
        run_from_key: Option<&'static str>,
        global: bool,
        impact_key: &'static str,
    ) -> Self {
        Self {
            method: CleanupMethod::NativeCommand,
            command: Some(CommandStep { program, args, run_from_key, global }),
            impact_key,
            method_key: "cleanup.method.nativeCommand",
        }
    }

    fn trash(impact_key: &'static str) -> Self {
        Self {
            method: CleanupMethod::TrashItem,
            command: None,
            impact_key,
            method_key: "cleanup.method.trashItem",
        }
    }
}

/// Format a command step for display, e.g. `cargo clean`.
pub fn display_command(step: &CommandStep) -> String {
    let mut out = String::from(step.program);
    for arg in step.args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// Look up the recommended cleanup plan for a candidate.
///
/// `path` is the candidate's display path. It disambiguates a project-local
/// dependency directory from a toolchain's global cache, which are cleaned
/// differently.
pub fn plan_for(kind: &CandidateKind, path: &str) -> CleanupPlan {
    match kind {
        CandidateKind::RebuildableCache { tool } => plan_for_tool(tool, path),
        CandidateKind::CacheDirectory { .. } => {
            CleanupPlan::trash("cleanup.impact.cacheRegenerated")
        }
        CandidateKind::PackageInstaller { .. } => {
            CleanupPlan::trash("cleanup.impact.installerGone")
        }
        CandidateKind::Archive { .. } => CleanupPlan::trash("cleanup.impact.archiveInTrash"),
        CandidateKind::StaleLargeFile => CleanupPlan::trash("cleanup.impact.fileInTrash"),
        CandidateKind::DuplicateGroup { .. } => {
            CleanupPlan::trash("cleanup.impact.duplicateInTrash")
        }
        CandidateKind::Trash => CleanupPlan {
            method: CleanupMethod::EmptyTrash,
            command: None,
            impact_key: "cleanup.impact.trashEmptied",
            method_key: "cleanup.method.emptyTrash",
        },
        CandidateKind::Log { .. } => CleanupPlan::trash("cleanup.impact.logRegenerated"),
        CandidateKind::TempFile => CleanupPlan::trash("cleanup.impact.tempGone"),
        CandidateKind::UserMarked => CleanupPlan::trash("cleanup.impact.markedInTrash"),
    }
}

fn plan_for_tool(tool: &str, path: &str) -> CleanupPlan {
    match tool {
        "cargo" => {
            // A project `target/` is maintained per-project; the global registry
            // is separate and is pruned with its own command.
            if is_global_cargo_path(path) {
                CleanupPlan::native(
                    "cargo",
                    &["cache", "--autoclean"],
                    None,
                    true,
                    "cleanup.impact.cargoRegistryPruned",
                )
            } else {
                CleanupPlan::native(
                    "cargo",
                    &["clean"],
                    Some("cleanup.runFrom.projectRoot"),
                    false,
                    "cleanup.impact.buildArtifactsRegenerated",
                )
            }
        }
        "npm" => {
            if is_global_npm_path(path) {
                CleanupPlan::native(
                    "npm",
                    &["cache", "clean", "--force"],
                    None,
                    true,
                    "cleanup.impact.npmCacheCleared",
                )
            } else {
                CleanupPlan::trash("cleanup.impact.depsReinstalled")
            }
        }
        "pnpm" | "pnpm-store" => CleanupPlan::native(
            "pnpm",
            &["store", "prune"],
            None,
            true,
            "cleanup.impact.pnpmStorePruned",
        ),
        "yarn" => CleanupPlan::native(
            "yarn",
            &["cache", "clean"],
            None,
            true,
            "cleanup.impact.yarnCacheCleared",
        ),
        "Gradle" => {
            if is_global_gradle_path(path) {
                CleanupPlan::trash("cleanup.impact.gradleCachesReDownload")
            } else {
                CleanupPlan::native(
                    "gradle",
                    &["clean"],
                    Some("cleanup.runFrom.projectRoot"),
                    false,
                    "cleanup.impact.buildArtifactsRegenerated",
                )
            }
        }
        "Homebrew" => CleanupPlan::native(
            "brew",
            &["cleanup", "--prune=all"],
            None,
            true,
            "cleanup.impact.brewOldDownloads",
        ),
        "pip" => CleanupPlan::native(
            "pip",
            &["cache", "purge"],
            None,
            true,
            "cleanup.impact.pipCachePurged",
        ),
        "Xcode" => CleanupPlan::trash("cleanup.impact.xcodeRebuildsIndex"),
        // Python bytecode, virtualenvs, mobile dependencies, terraform and any
        // other regenerable directory: move to trash; the next build/run recreates
        // what it needs.
        _ => CleanupPlan::trash("cleanup.impact.rebuildRegenerates"),
    }
}

fn is_global_cargo_path(path: &str) -> bool {
    path.contains("/.cargo/registry") || path.contains("/.cargo/git")
}

fn is_global_npm_path(path: &str) -> bool {
    path.contains("/.npm") || path.contains("/_cacache")
}

fn is_global_gradle_path(path: &str) -> bool {
    path.contains("/.gradle/caches") || path.contains("/.gradle/wrapper")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_target_uses_cargo_clean() {
        let plan = plan_for(
            &CandidateKind::RebuildableCache { tool: "cargo".into() },
            "~/code/app/target",
        );
        assert_eq!(plan.method, CleanupMethod::NativeCommand);
        let cmd = plan.command.clone().unwrap();
        assert_eq!(display_command(&cmd), "cargo clean");
        assert_eq!(cmd.run_from_key, Some("cleanup.runFrom.projectRoot"));
        assert!(!cmd.global);
    }

    #[test]
    fn global_cargo_registry_uses_autoclean() {
        let plan = plan_for(
            &CandidateKind::RebuildableCache { tool: "cargo".into() },
            "~/.cargo/registry/cache",
        );
        let cmd = plan.command.clone().unwrap();
        assert_eq!(display_command(&cmd), "cargo cache --autoclean");
        assert!(cmd.global);
    }

    #[test]
    fn homebrew_cache_uses_brew_cleanup() {
        let plan = plan_for(
            &CandidateKind::RebuildableCache { tool: "Homebrew".into() },
            "~/Library/Caches/Homebrew",
        );
        let cmd = plan.command.clone().unwrap();
        assert_eq!(display_command(&cmd), "brew cleanup --prune=all");
        assert!(cmd.global);
    }

    #[test]
    fn trash_candidate_empties_trash_not_a_command() {
        let plan = plan_for(&CandidateKind::Trash, "~/.Trash");
        assert_eq!(plan.method, CleanupMethod::EmptyTrash);
        assert!(plan.command.is_none());
    }

    #[test]
    fn archives_go_to_trash() {
        let plan = plan_for(
            &CandidateKind::Archive { extension: "zip".into() },
            "~/Downloads/bundle.zip",
        );
        assert_eq!(plan.method, CleanupMethod::TrashItem);
        assert_eq!(plan.impact_key, "cleanup.impact.archiveInTrash");
    }

    #[test]
    fn unknown_tool_falls_back_to_trash() {
        let plan = plan_for(
            &CandidateKind::RebuildableCache { tool: "Mystery".into() },
            "~/x/.mystery",
        );
        assert_eq!(plan.method, CleanupMethod::TrashItem);
        assert_eq!(plan.impact_key, "cleanup.impact.rebuildRegenerates");
    }
}
