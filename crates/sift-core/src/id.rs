//! Stable node and volume identity.
//!
//! The application needs identifiers that survive a rescan: a selection made
//! before a refresh must still name the same directory afterwards, and a
//! decision recorded in the habits store must still match. A path is the only
//! stable handle a filesystem gives us, so identity is derived from path bytes
//! — but with enough width that a collision cannot silently merge two nodes.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

const FNV_OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_OFFSET_B: u64 = 0x9e37_79b9_7f4a_7c15;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// A 128-bit content address for a path.
///
/// Two independent FNV-1a lanes with different seeds. At 10 million nodes the
/// collision probability stays far below hardware failure rates, and unlike a
/// truncating hash the failure mode is not "one disk tree silently overwrites
/// another".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeKey(u128);

impl NodeKey {
    #[inline]
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u128 {
        self.0
    }

    /// Parse the `n-<32 hex>` form produced by [`Display`](fmt::Display).
    ///
    /// Needed because a UI transports identity as text: an id sent to a front
    /// end and returned by it must round-trip back to the same node.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix("n-").unwrap_or(text);
        if digits.len() != 32 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(digits, 16).ok().map(Self)
    }

    /// Hash a path's bytes. Callers on Windows should normalise case first.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let a = fnv1a(FNV_OFFSET_A, bytes);
        let b = fnv1a(FNV_OFFSET_B, bytes);
        Self(((a as u128) << 64) | b as u128)
    }

    /// Hash a root plus one child name without re-walking the parent path.
    ///
    /// Children are always hashed from their full path bytes so a key means the
    /// same thing no matter which codepath produced it.
    pub fn from_path(path: &Path) -> Self {
        Self::from_bytes(&path_bytes(path))
    }

    /// Derive a child key by continuing both FNV lanes from this key's state.
    ///
    /// FNV-1a is order-serial, so the lanes after hashing the full parent path
    /// are exactly the state to continue from: append `"/"` (unless the parent
    /// path already ends in the separator) then the child name, and the result
    /// equals [`NodeKey::from_path`] for the joined path, without re-hashing
    /// every ancestor byte per entry.
    #[inline]
    pub fn child(self, name: &[u8], parent_path_ends_with_sep: bool) -> Self {
        let mut lane_a = (self.0 >> 64) as u64;
        let mut lane_b = self.0 as u64;

        let step = |lane: &mut u64, seed_prime: u64, bytes: &[u8]| {
            for &byte in bytes {
                *lane ^= byte as u64;
                *lane = lane.wrapping_mul(seed_prime);
            }
        };

        if !parent_path_ends_with_sep {
            step(&mut lane_a, FNV_PRIME, b"/");
            step(&mut lane_b, FNV_PRIME, b"/");
        }
        step(&mut lane_a, FNV_PRIME, name);
        step(&mut lane_b, FNV_PRIME, name);

        Self(((lane_a as u128) << 64) | lane_b as u128)
    }
}

/// Path bytes as the filesystem reports them: raw `OsStr` bytes on Unix, the
/// native wide encoding on Windows flattened to little-endian bytes.
pub fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut out = Vec::new();
        for unit in path.as_os_str().encode_wide() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "n-{:032x}", self.0)
    }
}

impl fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Identity of a mounted volume, derived from its mount point.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeId(u64);

impl VolumeId {
    pub fn from_mount_point(mount_point: &Path) -> Self {
        // A mount point is short; one lane is plenty and keeps the id readable.
        Self(fnv1a(FNV_OFFSET_A, &path_bytes(mount_point)))
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vol-{:x}", self.0)
    }
}

/// The file id (inode / file index) used for hardlink accounting.
///
/// `device` and `inode` together identify one physical file. `inode == 0` means
/// the platform did not report one and the file must never be deduplicated.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileId {
    pub device: u64,
    pub inode: u64,
}

impl FileId {
    #[inline]
    pub const fn new(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }

    /// Whether this id may be used to detect hardlinks.
    #[inline]
    pub const fn is_usable(&self) -> bool {
        self.inode != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_same_key() {
        let a = NodeKey::from_path(Path::new("/Users/me/Downloads"));
        let b = NodeKey::from_path(Path::new("/Users/me/Downloads"));
        assert_eq!(a, b);
    }

    #[test]
    fn different_paths_do_not_merge() {
        assert_ne!(
            NodeKey::from_path(Path::new("/a/b")),
            NodeKey::from_path(Path::new("/a/c"))
        );
        // The classic truncating-hash failure: shared prefix, different tail.
        assert_ne!(
            NodeKey::from_path(Path::new("/Users/me/Documents/report-final.pdf")),
            NodeKey::from_path(Path::new("/Users/me/Documents/report-final2.pdf"))
        );
    }

    #[test]
    fn display_and_parse_round_trip() {
        let key = NodeKey::from_bytes(b"/Users/me/Downloads");
        let text = key.to_string();
        assert_eq!(NodeKey::from_hex(&text), Some(key));
        // The bare hex is accepted too, so a front end may store either form.
        assert_eq!(NodeKey::from_hex(text.trim_start_matches("n-")), Some(key));
    }

    #[test]
    fn malformed_ids_are_rejected_rather_than_guessed() {
        assert!(NodeKey::from_hex("").is_none());
        assert!(NodeKey::from_hex("n-").is_none());
        assert!(NodeKey::from_hex("n-xyz").is_none());
        assert!(NodeKey::from_hex(&"n-".to_string().repeat(2)).is_none());
        // Right length, wrong alphabet.
        assert!(NodeKey::from_hex(&format!("n-{}", "z".repeat(32))).is_none());
    }

    #[test]
    fn key_is_128_bit() {
        let key = NodeKey::from_bytes(b"/x");
        assert_eq!(key.to_string().len(), 34);
        assert!(key.to_string().starts_with("n-"));
    }

    #[test]
    fn file_id_zero_inode_is_unusable() {
        assert!(!FileId::new(1, 0).is_usable());
        assert!(FileId::new(1, 42).is_usable());
    }

    #[test]
    fn volume_id_follows_mount_point() {
        let a = VolumeId::from_mount_point(Path::new("/"));
        let b = VolumeId::from_mount_point(Path::new("/Volumes/Backup"));
        let c = VolumeId::from_mount_point(Path::new("/"));
        assert_eq!(a, c);
        assert_ne!(a, b);
    }
}
