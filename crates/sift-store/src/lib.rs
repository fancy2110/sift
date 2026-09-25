//! Sift durable local state.
//!
//! Four documents, one owner:
//!
//! | Document | Holds | Purpose |
//! | --- | --- | --- |
//! | `verdicts.json` | fingerprint → verdict | make the next analysis cheap and stable |
//! | `cleanable.json` | the known-cleanable list | the standing answer to "what may be deleted?" |
//! | `decisions.json` | what the user chose | habit mining and suppressed suggestions |
//! | `settings.json` | preferences and thresholds | the monitor's trigger policy |
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
pub mod paths;
pub mod settings;
pub mod store;
pub mod verdicts;

pub use atomic::{read_document, write_atomic, write_document, LoadOutcome, SCHEMA_VERSION};
pub use cleanable::{CleanableEntry, CleanableList, RecordSummary, Upsert, DEFAULT_CLEANABLE_CAP};
pub use paths::{StorePaths, APP_DIR};
pub use settings::{
    AiSettings, AnalysisSettings, AutoCleanMode, MonitorSettings, Settings,
};
pub use store::Store;
pub use verdicts::{VerdictRecord, VerdictTable, DEFAULT_VERDICT_CAP};
