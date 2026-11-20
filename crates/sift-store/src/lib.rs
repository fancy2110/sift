//! Sift durable local state.
//!
//! One facade, several documents:
//!
//! | Document | Holds | Purpose |
//! | --- | --- | --- |
//! | `verdicts.json` | fingerprint → verdict | make the next analysis cheap and stable |
//! | `cleanable.json` | the known-cleanable list | the standing answer to "what may be deleted?" |
//! | `decisions.json` | what the user chose | habit mining and suppressed suggestions |
//! | `settings.json` | preferences and thresholds | the monitor's trigger policy |
//! | `history.json` | one row per cleanup session | the user-facing timeline |
//! | `routines.json` | accepted routines + dismissed suggestions | scheduled work definitions |
//! | `environment.json` | apps, package managers, projects, maintenance hints | environment-aware analysis |
//!
//! The scan journal (`scan-journal.sqlite`) and the size tree
//! (`size-tree.json`) live beside these documents but are owned by their own
//! modules; see `docs/architecture/core-data-flow.md`.
//!
//! Two rules shape the implementation:
//!
//! * **Every write is atomic** ([`atomic`]); a crash leaves the old file or the
//!   new one, never a truncated one.
//! * **A damaged document never blocks startup.** It is archived as `.corrupt`,
//!   defaults are used, and the caller is handed a warning to display.
//!
//! The crate owns no credentials: [`settings::AiSettings`] stores the *name* of
//! the environment variable an API key lives in, never the key.

pub mod atomic;
pub mod cleanable;
pub mod environment;
pub mod folders;
pub mod history;
pub mod journal_sqlite;
pub mod paths;
pub mod routines;
pub mod settings;
pub mod store;
pub mod verdicts;

pub use atomic::{
    read_document, write_atomic, write_document, LoadOutcome, ResetReason, StoreWarning,
    SCHEMA_VERSION,
};
pub use environment::{EnvironmentDoc, ENVIRONMENT_SCHEMA_VERSION};
pub use folders::{
    FolderChange, FolderEntry, FolderIndex, ListingState, ListingView, NodeView, SizeChange,
    SizeView, LISTING_TTL_MS, SIZE_TTL_MS,
};
pub use cleanable::{CleanableEntry, CleanableList, RecordSummary, Upsert, DEFAULT_CLEANABLE_CAP};
pub use history::{CleanupHistory, HistoryEntry, DEFAULT_HISTORY_CAP};
pub use journal_sqlite::SqliteScanJournal;
pub use paths::{StorePaths, APP_DIR};
pub use routines::{
    suggestion_key, RoutineDoc, RoutineEntry, RoutineList, RoutineMode, DEFAULT_ROUTINE_CAP,
};
pub use settings::{
    AiProfile, AiSettings, AnalysisSettings, AutoCleanMode, CleanupRule, MonitorSettings, Settings,
    MAX_AI_PROFILES,
};
pub use store::Store;
pub use verdicts::{VerdictRecord, VerdictTable, DEFAULT_VERDICT_CAP};
