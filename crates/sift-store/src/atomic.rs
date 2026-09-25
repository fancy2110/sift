//! Crash-safe file writes for local state.
//!
//! Two properties matter more than speed here:
//!
//! * **Atomic replacement.** A crash or a full disk must leave either the old
//!   file or the new one, never a half-written one. Every write goes to a
//!   temporary file in the same directory, is flushed, and is then renamed over
//!   the target — rename is atomic within a filesystem.
//! * **Corruption is survivable.** Local analysis state is a cache and a set of
//!   preferences. A truncated or hand-edited file must not stop the app from
//!   starting; it is moved aside as a `.corrupt` backup and a fresh document is
//!   used, and the caller is told so it can surface a warning.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The schema version of every document this crate writes.
///
/// Bumping it makes an older file get archived rather than misread, which is
/// the only safe behaviour when a field's meaning changes.
pub const SCHEMA_VERSION: u32 = 1;

/// A versioned document: a version tag plus a payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub version: u32,
    pub items: T,
}

/// What happened when a document was loaded.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadOutcome<T> {
    /// Read successfully.
    Loaded(T),
    /// No file yet: first run.
    Missing,
    /// Unusable (wrong version, truncated, not JSON). The old file was moved to
    /// `backup` when possible, and defaults should be used.
    Reset {
        reason: String,
        backup: Option<PathBuf>,
    },
}

impl<T> LoadOutcome<T> {
    /// Borrow the loaded value, leaving the outcome available for inspection.
    pub fn loaded(&self) -> Option<&T> {
        match self {
            LoadOutcome::Loaded(value) => Some(value),
            _ => None,
        }
    }

    /// Take the loaded value.
    pub fn into_loaded(self) -> Option<T> {
        match self {
            LoadOutcome::Loaded(value) => Some(value),
            _ => None,
        }
    }

    pub fn into_default(self, default: T) -> T {
        match self {
            LoadOutcome::Loaded(value) => value,
            _ => default,
        }
    }

    /// A warning suitable for the UI, or `None` when the load was clean.
    pub fn warning(&self, what: &str) -> Option<String> {
        match self {
            LoadOutcome::Reset { reason, backup } => Some(match backup {
                Some(path) => format!(
                    "{what} 无法读取（{reason}），已重置；原文件备份于 {}",
                    path.display()
                ),
                None => format!("{what} 无法读取（{reason}），已重置"),
            }),
            _ => None,
        }
    }
}

/// Write `bytes` to `path` atomically.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic write needs a parent directory",
        )
    })?;
    fs::create_dir_all(directory)?;

    // The temp file must share the directory so the rename stays within one
    // filesystem.
    let temp = directory.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sift".to_string()),
        std::process::id()
    ));

    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        // Durability: without this a power loss can leave the rename visible
        // with the contents still in the page cache.
        file.sync_all()?;
    }

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// Serialize and write a versioned document atomically.
pub fn write_document<T: Serialize>(path: &Path, items: &T) -> std::io::Result<()> {
    let envelope = Envelope {
        version: SCHEMA_VERSION,
        items,
    };
    let json = serde_json::to_vec_pretty(&envelope)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    write_atomic(path, &json)
}

/// Load a versioned document, tolerating every failure mode.
///
/// Never returns `Err`: a caller starting the app should get a value or a
/// reason to reset, not an error to handle.
pub fn read_document<T: DeserializeOwned>(path: &Path) -> LoadOutcome<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(err) => {
            return LoadOutcome::Reset {
                reason: err.to_string(),
                backup: None,
            }
        }
    };

    let envelope = match serde_json::from_slice::<Envelope<T>>(&bytes) {
        Ok(envelope) => envelope,
        Err(err) => {
            return LoadOutcome::Reset {
                reason: format!("JSON 解析失败：{err}"),
                backup: archive_corrupt(path),
            }
        }
    };

    if envelope.version != SCHEMA_VERSION {
        return LoadOutcome::Reset {
            reason: format!(
                "schema 版本 {} 与当前 {} 不一致",
                envelope.version, SCHEMA_VERSION
            ),
            backup: archive_corrupt(path),
        };
    }

    LoadOutcome::Loaded(envelope.items)
}

/// Move an unusable file aside so the next load starts clean without losing
/// whatever the user had.
fn archive_corrupt(path: &Path) -> Option<PathBuf> {
    let backup = path.with_extension(format!(
        "{}.corrupt",
        path.extension()
            .map(|ext| ext.to_string_lossy().into_owned())
            .unwrap_or_else(|| "json".to_string())
    ));
    fs::rename(path, &backup).ok().map(|_| backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-store-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_a_document() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("doc.json");
        write_document(&path, &vec!["a".to_string(), "b".to_string()]).unwrap();
        let loaded: LoadOutcome<Vec<String>> = read_document(&path);
        assert!(loaded.warning("x").is_none());
        assert_eq!(loaded.into_loaded().unwrap(), vec!["a", "b"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_a_first_run_not_a_failure() {
        let dir = temp_dir("missing");
        let loaded: LoadOutcome<Vec<String>> = read_document(&dir.join("nope.json"));
        assert!(loaded.loaded().is_none());
        assert!(loaded.warning("x").is_none());
        assert!(matches!(loaded, LoadOutcome::Missing));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_archived_and_reported() {
        let dir = temp_dir("corrupt");
        let path = dir.join("doc.json");
        fs::write(&path, b"{ this is not json").unwrap();

        let loaded: LoadOutcome<Vec<String>> = read_document(&path);
        match &loaded {
            LoadOutcome::Reset { backup, .. } => {
                let backup = backup.as_ref().expect("a backup path");
                assert!(backup.exists(), "the original must be preserved");
                assert!(!path.exists(), "the bad file must be moved aside");
            }
            other => panic!("expected a reset, got {other:?}"),
        }
        assert!(loaded.warning("分析结论").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_schema_version_is_archived_not_misread() {
        let dir = temp_dir("version");
        let path = dir.join("doc.json");
        fs::write(&path, br#"{"version":999,"items":["future"]}"#).unwrap();
        let loaded: LoadOutcome<Vec<String>> = read_document(&path);
        assert!(loaded.loaded().is_none());
        assert!(matches!(loaded, LoadOutcome::Reset { .. }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_behind() {
        let dir = temp_dir("temp");
        let path = dir.join("doc.json");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "a temp file was left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_over_an_existing_file_replaces_it_wholesale() {
        let dir = temp_dir("replace");
        let path = dir.join("doc.json");
        write_atomic(&path, b"aaaaaaaaaaaaaaaaaaaa").unwrap();
        write_atomic(&path, b"bb").unwrap();
        // A non-atomic write would leave bytes of the old longer content.
        assert_eq!(fs::read(&path).unwrap(), b"bb");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unwritable_target_is_an_error_not_a_silent_loss() {
        let dir = temp_dir("badpath");
        // A path whose parent is a file cannot be created.
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let target = blocker.join("nested").join("doc.json");
        assert!(write_atomic(&target, b"data").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
