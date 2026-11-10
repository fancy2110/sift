//! Environment inventory and maintenance knowledge for Sift.
//!
//! The crate answers three questions for a scan:
//!
//! 1. Which desktop applications, package managers and projects exist?
//! 2. Which maintenance actions are plausible for those package managers?
//! 3. What would an app uninstall plan contain, before anything is deleted?
//!
//! It performs no deletion and starts no external process beyond bounded,
//! read-only probes (`--version`, `plutil`). Execution stays in `sift-platform`
//! / the Tauri layer behind explicit confirmation.

pub mod context;
pub mod inventory;
pub mod maintenance;
pub mod pm;
pub mod plan;
pub mod quarantine;
pub mod recipes;

pub use context::{redact_path, AppFact, PmFact, ProjectFact, ScanContext, SCHEMA_VERSION};
pub use inventory::detect;
pub use pm::{plan_action, run_action, Destructiveness, PmActionPlan, PmRunReport};
pub use maintenance::{
    hints_for, HintSource, HintStatus, MaintenanceHint, ACTION_CLEAN_CACHE, ACTION_DEDUPE,
    ACTION_PRUNE_STORE, ACTION_REMOVE_OLD_VERSION, ACTION_REMOVE_ORPHAN, ACTION_VERIFY_INTEGRITY,
};
pub use recipes::{homebrew_zap_paths, parse_zap_json, RecipeArtifact};
pub use plan::{
    plan_uninstall, plan_uninstall_with_recipes, AppUninstallPlan, ArtifactKind, ArtifactScope, Disposition, UninstallArtifact,
};
pub use quarantine::{
    execute as execute_uninstall, restore as restore_uninstall, ManifestEntry, RestoreReport,
    UninstallManifest,
};
