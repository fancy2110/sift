//! Byte accounting.
//!
//! A disk-space manager has to answer two different questions, and conflating
//! them makes the numbers wrong on APFS, Btrfs and NTFS-compressed volumes:
//!
//! * *logical* size — `st_size`, the bytes the file claims. This is what a user
//!   sees in Finder/Explorer and what a treemap should lay out.
//! * *physical* size — the bytes the volume actually spends, i.e. allocated
//!   blocks or the compressed/cloned extent size.
//!
//! APFS clones and sparse files make logical ≫ physical; compressed files make
//! the reverse. Both are tracked so the UI can explain the gap instead of
//! looking like it cannot add up.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{AddAssign, SubAssign};

/// An exact byte count split into logical and physical totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ByteSize {
    /// Sum of file `st_size` values.
    pub logical: u64,
    /// Sum of allocated bytes (`st_blocks * 512`, compressed size, …).
    /// Zero when the platform did not report it.
    pub physical: u64,
}

impl ByteSize {
    pub const ZERO: Self = Self {
        logical: 0,
        physical: 0,
    };

    #[inline]
    pub const fn new(logical: u64, physical: u64) -> Self {
        Self { logical, physical }
    }

    /// A measurement where only the logical size is known.
    #[inline]
    pub const fn logical_only(logical: u64) -> Self {
        Self {
            logical,
            physical: 0,
        }
    }

    /// The size to lay out and sort by.
    ///
    /// This is a disk-space tool, so the bytes the volume actually spends win
    /// whenever the platform reported them: a 64 GB VM image that occupies
    /// 8 GB counts as 8 GB, and a sparse file counts by its extents. Logical
    /// size remains the fallback when allocation was not reported.
    #[inline]
    pub const fn dominant(self) -> u64 {
        if self.physical > 0 {
            self.physical
        } else {
            self.logical
        }
    }

    /// Physical bytes reclaimable by deleting this. Falls back to logical when
    /// the platform reported no allocation.
    #[inline]
    pub const fn reclaimable(self) -> u64 {
        if self.physical > 0 {
            self.physical
        } else {
            self.logical
        }
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.logical == 0 && self.physical == 0
    }
}

impl AddAssign for ByteSize {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.logical = self.logical.saturating_add(rhs.logical);
        self.physical = self.physical.saturating_add(rhs.physical);
    }
}

impl SubAssign for ByteSize {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.logical = self.logical.saturating_sub(rhs.logical);
        self.physical = self.physical.saturating_sub(rhs.physical);
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_bytes(self.logical))
    }
}

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// Format a byte count the way the rest of the product does: binary units,
/// no trailing `.0`, and a space before the unit.
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    format!("{:.*} {}", digits, value, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_prefers_physical_allocation() {
        // A VM/sparse file claims 100 logical bytes but occupies 4096:
        // real disk usage is what the product counts.
        assert_eq!(ByteSize::new(100, 4096).dominant(), 4096);
        assert_eq!(ByteSize::logical_only(0).dominant(), 0);
        // Allocation unknown: logical is all there is.
        assert_eq!(ByteSize::logical_only(100).dominant(), 100);
        assert_eq!(ByteSize::new(0, 4096).dominant(), 4096);
    }

    #[test]
    fn reclaimable_prefers_physical() {
        // A cloned 1 GiB file that occupies one block: deleting frees the block.
        let cloned = ByteSize::new(1 << 30, 4096);
        assert_eq!(cloned.reclaimable(), 4096);
        assert_eq!(ByteSize::logical_only(1 << 30).reclaimable(), 1 << 30);
    }

    #[test]
    fn add_accumulates_both_axes() {
        let mut total = ByteSize::ZERO;
        total += ByteSize::new(10, 4096);
        total += ByteSize::new(20, 4096);
        assert_eq!(total, ByteSize::new(30, 8192));
    }

    #[test]
    fn add_saturates_instead_of_wrapping() {
        let mut total = ByteSize::new(u64::MAX, 0);
        total += ByteSize::new(10, 0);
        assert_eq!(total.logical, u64::MAX);
    }

    #[test]
    fn formatting_matches_product_language() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(200 * 1024 * 1024 * 1024), "200 GB");
    }
}
