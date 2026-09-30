//! Where local state lives.

use std::path::PathBuf;

/// The directory layout of Sift's local state.
///
/// Uses the platform's per-user data directory rather than the home root:
/// `~/Library/Application Support/Sift` on macOS, `%APPDATA%\Sift` on Windows,
/// `$XDG_DATA_HOME/sift` on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    root: PathBuf,
}

/// The application directory name inside the platform data directory.
pub const APP_DIR: &str = "Sift";

impl StorePaths {
    /// Discover the platform data directory. `None` when the platform gives us
    /// nowhere to write, in which case the caller should run without
    /// persistence rather than fail.
    pub fn discover() -> Option<Self> {
        dirs::data_dir().map(|base| Self {
            root: base.join(APP_DIR),
        })
    }

    /// A store rooted at an explicit directory, for tests and portable installs.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Cached verdicts, keyed by path fingerprint.
    pub fn verdicts(&self) -> PathBuf {
        self.root.join("verdicts.json")
    }

    /// The known-cleanable list: the persisted basis for later analysis and
    /// unattended cleanup.
    pub fn cleanable(&self) -> PathBuf {
        self.root.join("cleanable.json")
    }

    /// The decision log used for habit mining.
    pub fn decisions(&self) -> PathBuf {
        self.root.join("decisions.json")
    }

    /// User preferences, including the monitor thresholds.
    ///
    /// Deliberately never contains a credential: see
    /// [`crate::settings::AiSettings`].
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// The user-facing cleanup timeline, one session per finished cleanup.
    pub fn history(&self) -> PathBuf {
        self.root.join("history.json")
    }

    /// Saved routines accepted from habit mining.
    pub fn routines(&self) -> PathBuf {
        self.root.join("routines.json")
    }

    /// SQLite database holding resumable scan metadata.
    pub fn scan_journal(&self) -> PathBuf {
        self.root.join("scan-journal.sqlite")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_share_the_root_and_are_distinct() {
        let paths = StorePaths::at("/tmp/sift-test");
        let all = [
            paths.verdicts(),
            paths.cleanable(),
            paths.decisions(),
            paths.settings(),
            paths.history(),
            paths.routines(),
        ];
        for path in &all {
            assert_eq!(path.parent().unwrap(), paths.root());
            assert_eq!(path.extension().unwrap(), "json");
        }
        let mut unique = all.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "paths must not collide");
    }

    #[test]
    fn discovery_is_under_a_data_directory_when_available() {
        // No platform data dir is acceptable; persistence is optional.
        if let Some(paths) = StorePaths::discover() {
            assert!(paths.root().ends_with(APP_DIR));
            assert!(paths.root().is_absolute());
        }
    }
}
