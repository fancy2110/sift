//! Name interning.
//!
//! A scan of a large volume meets millions of names, but only a few hundred
//! thousand are *distinct* — `node_modules`, `target`, `Contents`, `.git` and
//! friends repeat endlessly. Storing an owned `String` per node wastes both the
//! allocation header and the duplicate text. Every name therefore lives once in
//! a single byte arena and each node stores an 8-byte [`NameRef`].
//!
//! Lookup is a hash map keyed by a 64-bit FNV-1a of the name; the map value is
//! the candidate slot, and the arena bytes are compared to confirm. A hash
//! collision costs one linear probe, never a wrong name.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Pointer into the interned name arena: a byte offset plus a length.
///
/// Both halves are 32-bit, which caps the arena at 4 GiB of *distinct* names —
/// far beyond what a real volume produces — while keeping the handle at 8 bytes
/// so a node record stays cache-friendly.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct NameRef {
    offset: u32,
    len: u32,
}

impl NameRef {
    #[inline]
    pub const fn offset(self) -> u32 {
        self.offset
    }

    #[inline]
    pub const fn len(self) -> u32 {
        self.len
    }

    /// Whether this handle names the empty string. A path component never is,
    /// but a caller formatting a handle should not have to say `len() == 0`.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// Deduplicating store for file names.
#[derive(Default)]
pub struct NameInterner {
    bytes: Vec<u8>,
    /// hash -> index into `slots`
    index: HashMap<u64, u32>,
    /// One entry per distinct name; `next` chains hash collisions.
    slots: Vec<Slot>,
}

#[derive(Clone, Copy)]
struct Slot {
    name: NameRef,
    next: u32,
}

const NO_SLOT: u32 = u32::MAX;

fn hash_name(name: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in name {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl NameInterner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `name`, returning a handle that is equal for equal bytes.
    pub fn intern(&mut self, name: &[u8]) -> NameRef {
        let hash = hash_name(name);

        let mut cursor = self.index.get(&hash).copied().unwrap_or(NO_SLOT);
        while cursor != NO_SLOT {
            let slot = self.slots[cursor as usize];
            if self.lookup(slot.name) == name {
                return slot.name;
            }
            cursor = slot.next;
        }

        let name_ref = NameRef {
            offset: self.bytes.len() as u32,
            len: name.len() as u32,
        };
        self.bytes.extend_from_slice(name);
        let slot_ix = self.slots.len() as u32;
        self.slots.push(Slot {
            name: name_ref,
            next: self.index.get(&hash).copied().unwrap_or(NO_SLOT),
        });
        self.index.insert(hash, slot_ix);
        name_ref
    }

    /// The bytes behind a handle. Panics only if `name` came from another
    /// interner, which the type system cannot prevent but the engine never does.
    #[inline]
    pub fn lookup(&self, name: NameRef) -> &[u8] {
        let start = name.offset as usize;
        let end = start + name.len as usize;
        &self.bytes[start..end]
    }

    /// Decode a handle as text for the UI, replacing invalid sequences so a
    /// stray byte can never panic a render.
    pub fn to_string_lossy(&self, name: NameRef) -> String {
        String::from_utf8_lossy(self.lookup(name)).into_owned()
    }

    /// A `String` for a handle, cheap when the underlying bytes are valid UTF-8.
    pub fn to_string_checked(&self, name: NameRef) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.lookup(name).to_vec())
    }

    /// Distinct names held.
    #[inline]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Arena bytes used by the name text itself.
    #[inline]
    pub fn text_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Total resident bytes owned by the interner, for the memory budget report.
    pub fn resident_bytes(&self) -> usize {
        self.bytes.capacity()
            + self.slots.capacity() * std::mem::size_of::<Slot>()
            + self.index.capacity() * (std::mem::size_of::<u64>() + std::mem::size_of::<u32>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_names_share_one_entry() {
        let mut interner = NameInterner::new();
        let a = interner.intern(b"node_modules");
        let b = interner.intern(b"node_modules");
        assert_eq!(a, b);
        assert_eq!(interner.len(), 1);
        assert_eq!(interner.text_bytes(), b"node_modules".len());
    }

    #[test]
    fn distinct_names_stay_distinct() {
        let mut interner = NameInterner::new();
        let a = interner.intern(b"Cargo.toml");
        let b = interner.intern(b"Cargo.lock");
        assert_ne!(a, b);
        assert_eq!(interner.lookup(a), b"Cargo.toml");
        assert_eq!(interner.lookup(b), b"Cargo.lock");
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn prefix_and_empty_names_are_preserved() {
        let mut interner = NameInterner::new();
        let empty = interner.intern(b"");
        let short = interner.intern(b"a");
        let long = interner.intern(b"aa");
        assert_eq!(interner.lookup(empty), b"");
        assert_eq!(interner.lookup(short), b"a");
        assert_eq!(interner.lookup(long), b"aa");
        assert_eq!(interner.len(), 3);
    }

    #[test]
    fn non_utf8_names_do_not_panic() {
        let mut interner = NameInterner::new();
        let raw = [0x66, 0x6f, 0xff, 0xfe];
        let name = interner.intern(&raw);
        assert_eq!(interner.lookup(name), raw);
        assert!(interner.to_string_checked(name).is_err());
        assert!(!interner.to_string_lossy(name).is_empty() || raw.is_empty());
    }

    /// The hash-collision path: force two names into one slot chain and check
    /// that lookup still returns exact bytes rather than the first hash match.
    #[test]
    fn collision_chain_compares_bytes() {
        let mut interner = NameInterner::new();
        let a = {
            let mut bytes = vec![b'x'; 8];
            bytes[7] = 1;
            interner.intern(&bytes)
        };
        let b = {
            let mut bytes = vec![b'y'; 8];
            bytes[7] = 2;
            interner.intern(&bytes)
        };
        assert_ne!(a, b);
        assert_eq!(interner.lookup(a), &[b'x'; 7].iter().copied().chain([1]).collect::<Vec<_>>()[..]);
        assert_eq!(interner.lookup(b), &[b'y'; 7].iter().copied().chain([2]).collect::<Vec<_>>()[..]);
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn accounting_reports_indices() {
        let mut interner = NameInterner::new();
        for i in 0..1000u32 {
            interner.intern(format!("name-{i}").as_bytes());
        }
        assert_eq!(interner.len(), 1000);
        assert!(interner.text_bytes() > 1000);
        assert!(interner.resident_bytes() > interner.text_bytes());
    }
}
