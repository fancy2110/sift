//! Storage for the AI provider's API token.
//!
//! The shared `sift-store` crate is deliberately credential-free: its settings
//! name an environment variable rather than holding a key, so a disk cleaner
//! never writes a bearer token next to the file list it is analyzing. A GUI app
//! launched from Finder has no shell environment though, so this adapter keeps
//! the token in the user's **macOS Keychain** (generic password, service
//! `Sift`), where it is encrypted at rest and never lands in `settings.json`.
//!
//! Other platforms fall back to a user-private (`0600`) file; that path exists
//! so the crate compiles and stays usable off macOS, not as a security equal.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::ptr;

    type OsStatus = i32;

    const ERR_SEC_SUCCESS: OsStatus = 0;
    const ERR_SEC_ITEM_NOT_FOUND: OsStatus = -25300;

    const SERVICE: &str = "Sift";
    const ACCOUNT: &str = "ai-api-key";

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainFindGenericPassword(
            keychain_or_array: *const c_void,
            service_name_length: u32,
            service_name: *const u8,
            account_name_length: u32,
            account_name: *const u8,
            password_length: *mut u32,
            password_data: *mut *mut u8,
            item_ref: *mut *const c_void,
        ) -> OsStatus;
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_name_length: u32,
            service_name: *const u8,
            account_name_length: u32,
            account_name: *const u8,
            password_length: u32,
            password_data: *const u8,
            item_ref: *mut *const c_void,
        ) -> OsStatus;
        fn SecKeychainItemDelete(item_ref: *const c_void) -> OsStatus;
        fn SecKeychainItemFreeContent(
            attr_list: *const c_void,
            data: *mut c_void,
        ) -> OsStatus;
        fn CFRelease(cf_ref: *const c_void);
    }

    /// Find the stored item, optionally returning its secret.
    ///
    /// Any returned item ref is owned by the caller and must be `CFRelease`d;
    /// any returned password bytes must be `SecKeychainFreePassword`d.
    unsafe fn find_item(
        want_password: bool,
    ) -> Result<Option<(Option<String>, *const c_void)>, String> {
        let mut password_len: u32 = 0;
        let mut password_data: *mut u8 = ptr::null_mut();
        let mut item_ref: *const c_void = ptr::null();

        let status = SecKeychainFindGenericPassword(
            ptr::null(), // default keychain search list
            SERVICE.len() as u32,
            SERVICE.as_ptr(),
            ACCOUNT.len() as u32,
            ACCOUNT.as_ptr(),
            if want_password {
                &mut password_len
            } else {
                ptr::null_mut()
            },
            if want_password {
                &mut password_data
            } else {
                ptr::null_mut()
            },
            &mut item_ref,
        );

        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(None);
        }
        if status != ERR_SEC_SUCCESS {
            return Err(format!("keychain find failed (OSStatus {status})"));
        }

        let secret = if want_password && !password_data.is_null() {
            let bytes = std::slice::from_raw_parts(password_data, password_len as usize);
            let value = String::from_utf8(bytes.to_vec()).ok();
            SecKeychainItemFreeContent(ptr::null(), password_data as *mut c_void);
            value
        } else {
            None
        };
        Ok(Some((secret, item_ref)))
    }

    pub fn token() -> Option<String> {
        let found = unsafe { find_item(true) }.ok()??;
        let item_ref = found.1;
        if !item_ref.is_null() {
            unsafe { CFRelease(item_ref) };
        }
        found.0
    }

    pub fn has_token() -> bool {
        let found = unsafe { find_item(false) };
        let Ok(Some((_, item_ref))) = found else {
            return false;
        };
        if !item_ref.is_null() {
            unsafe { CFRelease(item_ref) };
        }
        true
    }

    pub fn set_token(token: &str) -> Result<(), String> {
        // Replace in place: delete the old item if one exists, then add.
        if let Some((_, item_ref)) = unsafe { find_item(false) }? {
            if !item_ref.is_null() {
                let status = unsafe { SecKeychainItemDelete(item_ref) };
                unsafe { CFRelease(item_ref) };
                if status != ERR_SEC_SUCCESS {
                    return Err(format!("keychain delete failed (OSStatus {status})"));
                }
            }
        }

        let status = unsafe {
            SecKeychainAddGenericPassword(
                ptr::null(), // default keychain
                SERVICE.len() as u32,
                SERVICE.as_ptr(),
                ACCOUNT.len() as u32,
                ACCOUNT.as_ptr(),
                token.len() as u32,
                token.as_ptr(),
                ptr::null_mut(),
            )
        };
        if status != ERR_SEC_SUCCESS {
            return Err(format!("keychain add failed (OSStatus {status})"));
        }
        Ok(())
    }

    pub fn delete_token() -> Result<(), String> {
        if let Some((_, item_ref)) = unsafe { find_item(false) }? {
            if !item_ref.is_null() {
                let status = unsafe { SecKeychainItemDelete(item_ref) };
                unsafe { CFRelease(item_ref) };
                if status != ERR_SEC_SUCCESS {
                    return Err(format!("keychain delete failed (OSStatus {status})"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::fs;
    use std::path::PathBuf;

    fn token_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|dir| dir.join("Sift").join("ai-token"))
    }

    pub fn token() -> Option<String> {
        let path = token_path()?;
        fs::read_to_string(path).ok().map(|value| value.trim().to_owned())
    }

    pub fn has_token() -> bool {
        token().is_some()
    }

    pub fn set_token(token: &str) -> Result<(), String> {
        let path = token_path().ok_or_else(|| "no data directory".to_string())?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|err| err.to_string())?;
        fs::write(&path, token).map_err(|err| err.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn delete_token() -> Result<(), String> {
        if let Some(path) = token_path() {
            if path.exists() {
                fs::remove_file(path).map_err(|err| err.to_string())?;
            }
        }
        Ok(())
    }
}

pub use imp::{delete_token, has_token, set_token, token};
