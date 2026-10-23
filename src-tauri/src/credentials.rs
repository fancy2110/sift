//! Storage for the AI provider's API tokens.
//!
//! The shared `sift-store` crate is deliberately credential-free: its settings
//! name an environment variable rather than holding a key, so a disk cleaner
//! never writes a bearer token next to the file list it is analyzing. This
//! adapter keeps one token per provider profile in a user-private (`0600`)
//! file under the app data directory, e.g.
//! `~/Library/Application Support/Sift/ai-token-ai-api-key-profile-1`.
//!
//! The macOS Keychain was used before. It was dropped because a locally built
//! binary is re-signed on every rebuild, which invalidates the item's ACL:
//! reads either fail with `errSecAuthFailed` (-25293) or block on a system
//! prompt, freezing the request. A `0600` file has neither failure mode, and
//! the settings sheet is the only place that writes a token.

use std::fs;
use std::path::PathBuf;

const LEGACY_ACCOUNT: &str = "ai-api-key";

/// File name for one account. The legacy single-profile account keeps the
/// unprefixed name so an existing key survives the upgrade.
fn token_path(account: &str) -> Option<PathBuf> {
    let name = if account == LEGACY_ACCOUNT {
        "ai-token".to_string()
    } else {
        let safe: String = account
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        format!("ai-token-{safe}")
    };
    dirs::data_local_dir().map(|dir| dir.join("Sift").join(name))
}

/// Storage account for one provider profile.
pub fn account_for(profile_id: &str) -> String {
    format!("ai-api-key:{profile_id}")
}

/// The account used by the single-profile builds, read for migration.
pub fn legacy_account() -> &'static str {
    LEGACY_ACCOUNT
}

pub fn token_result(account: &str) -> Result<Option<String>, String> {
    let Some(path) = token_path(account) else {
        return Ok(None);
    };
    match fs::read_to_string(&path) {
        Ok(value) => {
            let value = value.trim().to_string();
            Ok(if value.is_empty() { None } else { Some(value) })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read token failed: {error}")),
    }
}

pub fn has_token(account: &str) -> bool {
    matches!(token_result(account), Ok(Some(_)))
}

pub fn set_token(account: &str, token: &str) -> Result<(), String> {
    let path = token_path(account).ok_or_else(|| "no data directory".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        // Create with 0600 before writing so the secret is never briefly
        // world-readable.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| error.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, token).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn delete_token(account: &str) -> Result<(), String> {
    let Some(path) = token_path(account) else {
        return Ok(());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("delete token failed: {error}")),
    }
}
