//! Bulk directory reading.
//!
//! The whole 60-second budget lives or dies here. A naive walk costs one
//! `lstat` syscall per file; this module turns "read a directory" into a small
//! number of batched syscalls that return every entry together with its
//! metadata. On macOS that is `getattrlistbulk(2)`; everywhere else the module
//! degrades to `read_dir` + one `symlink_metadata` per entry, which is correct
//! and bounded, just slower.
//!
//! Every implementation returns the same [`RawEntry`] records so the scan
//! engine is identical above this seam. The fast path is verified against the
//! portable path by tests that run on a real directory tree.

use std::fs;
use std::io;
use std::path::Path;

use sift_core::id::FileId;

/// One directory entry as the platform reported it, before any tree
/// bookkeeping. Raw bytes stay raw: decoding names is the UI's job.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RawEntry {
    /// File name bytes exactly as the filesystem returned them.
    pub name: Vec<u8>,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_file: bool,
    /// `st_size` — the bytes the file claims.
    pub logical_size: u64,
    /// Allocated bytes (`st_blocks * 512`). `0` when not reported.
    pub physical_size: u64,
    /// Modification time in unix milliseconds. `0` when not reported.
    pub mtime_ms: i64,
    /// `st_dev` — `0` when not reported.
    pub dev: u64,
    /// `st_ino` — `0` when not reported. Shared across hardlinks.
    pub ino: u64,
    /// Hard link count. `0` means the platform did not report one.
    ///
    /// Carried because hardlink accounting must only remember files that
    /// actually have more than one link: inserting every file into the dedupe
    /// table costs roughly 40 bytes each, which on a volume with millions of
    /// files is tens of megabytes spent to detect a case that is rare.
    pub nlink: u32,
}

impl RawEntry {
    /// A usable hardlink identity, when the platform reported one.
    pub fn file_id(&self) -> Option<FileId> {
        if self.dev != 0 && self.ino != 0 {
            Some(FileId::new(self.dev, self.ino))
        } else {
            None
        }
    }

    /// The size to aggregate. Physical wins when known and smaller (clones,
    /// sparse files, compression); logical otherwise. The tree keeps both
    /// separately; this is only for callers that need one number.
    pub fn size(&self) -> sift_core::ByteSize {
        sift_core::ByteSize::new(self.logical_size, self.physical_size)
    }
}

/// Which implementation produced the entries. Used for diagnostics and so a
/// benchmark can prove the fast path is actually being taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReaderKind {
    /// `std::fs::read_dir` + per-entry metadata.
    Portable,
    /// macOS `getattrlistbulk(2)`.
    GetAttrListBulk,
}

impl ReaderKind {
    pub fn is_fast(&self) -> bool {
        !matches!(self, ReaderKind::Portable)
    }
}

/// The result of reading one directory.
#[derive(Debug)]
#[non_exhaustive]
pub struct DirReader {
    kind: ReaderKind,
    entries: Vec<RawEntry>,
}

impl DirReader {
    /// Read `dir`, preferring the fastest platform bulk enumeration.
    ///
    /// The fast path is only a performance upgrade; any failure inside it falls
    /// back to the portable path, so this function returns `Ok` for any
    /// directory that `read_dir` can open.
    pub fn read(dir: &Path, want_physical: bool) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            match macos::read_bulk(dir, want_physical) {
                Ok(entries) => {
                    return Ok(Self {
                        kind: ReaderKind::GetAttrListBulk,
                        entries,
                    });
                }
                Err(err) => {
                    if !err.is_fallback() {
                        return Err(err.into_io());
                    }
                    // Unsupported or unreadable via the bulk path: fall through.
                }
            }
        }
        Self::read_portable(dir, want_physical)
    }

    /// The portable implementation, exposed so tests and benchmarks can compare
    /// it against the fast path on identical input.
    pub fn read_portable(dir: &Path, want_physical: bool) -> io::Result<Self> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(_) => continue, // vanished between readdir and stat
            };
            let file_type = meta.file_type();
            entries.push(RawEntry {
                name: entry.file_name().as_encoded_bytes().to_vec(),
                is_dir: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
                is_file: file_type.is_file(),
                logical_size: meta.len(),
                physical_size: if want_physical {
                    allocated_bytes(&meta)
                } else {
                    0
                },
                mtime_ms: modified_ms(&meta),
                dev: device_of(&meta),
                ino: inode_of(&meta),
                nlink: links_of(&meta),
            });
        }
        Ok(Self {
            kind: ReaderKind::Portable,
            entries,
        })
    }

    #[inline]
    pub fn kind(&self) -> ReaderKind {
        self.kind
    }

    #[inline]
    pub fn entries(&self) -> &[RawEntry] {
        &self.entries
    }

    #[inline]
    pub fn into_entries(self) -> Vec<RawEntry> {
        self.entries
    }
}

// ---- per-platform metadata extraction (portable path) ----------------------

#[cfg(unix)]
pub(crate) fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
pub(crate) fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    let _ = meta;
    0
}

#[cfg(unix)]
pub(crate) fn modified_ms(meta: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime().saturating_mul(1000).saturating_add(meta.mtime_nsec() / 1_000_000)
}

#[cfg(windows)]
pub(crate) fn modified_ms(meta: &fs::Metadata) -> i64 {
    use std::os::windows::fs::MetadataExt;
    meta.last_write_time()
        .map(|ft| ((ft as i128 - 116_444_736_000_000_000) / 10_000) as i64)
        .unwrap_or(0)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn modified_ms(_meta: &fs::Metadata) -> i64 {
    0
}

#[cfg(unix)]
pub(crate) fn device_of(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(windows)]
pub(crate) fn device_of(meta: &fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    meta.volume_serial_number().unwrap_or(0) as u64
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn device_of(_meta: &fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
pub(crate) fn inode_of(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

/// Hard link count, or `0` when the platform does not report one.
#[cfg(unix)]
pub(crate) fn links_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink().min(u32::MAX as u64) as u32
}

#[cfg(windows)]
pub(crate) fn links_of(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.number_of_links().unwrap_or(0)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn links_of(_meta: &fs::Metadata) -> u32 {
    0
}

#[cfg(windows)]
pub(crate) fn inode_of(meta: &fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    meta.file_index().unwrap_or(0)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn inode_of(_meta: &fs::Metadata) -> u64 {
    0
}

// ---- macOS: getattrlistbulk -------------------------------------------------

/// The macOS fast path is isolated here. It returns a dedicated error type so
/// the caller can distinguish "not supported, fall back" from "real I/O error".
#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    // --- attribute bits (from <sys/attr.h>; libc exposes these as constants) ---
    use libc::{
        attrlist, ATTR_CMN_DEVID, ATTR_CMN_FILEID, ATTR_CMN_MODTIME, ATTR_CMN_NAME,
        ATTR_CMN_OBJTYPE, ATTR_CMN_RETURNED_ATTRS, ATTR_FILE_DATAALLOCSIZE, ATTR_FILE_DATALENGTH,
        ATTR_FILE_LINKCOUNT, ATTR_BIT_MAP_COUNT,
    };

    /// Values of `enum vtype` from `<sys/vnode.h>`:
    /// VNON=0, VREG=1, VDIR=2, VBLK=3, VCHR=4, VLNK=5.
    const VREG: u32 = 0x1;
    const VDIR: u32 = 0x2;
    const VLNK: u32 = 0x5;

    /// macOS returns entries in groups. A flat directory can hold hundreds of
    /// thousands of entries; a large buffer cuts the number of
    /// `getattrlistbulk` round trips proportionally. 4 MB stays cheap while
    /// amortising the per-syscall latency on giant directories.
    const CHUNK_BYTES: usize = 4 * 1024 * 1024;

    /// Errors from the fast path. `Fallback` means "use the portable path".
    #[derive(Debug)]
    pub(super) enum BulkError {
        Fallback(&'static str),
        Io(io::Error),
    }

    impl BulkError {
        pub(super) fn is_fallback(&self) -> bool {
            matches!(self, BulkError::Fallback(_))
        }

        pub(super) fn into_io(self) -> io::Error {
            match self {
                BulkError::Fallback(why) => io::Error::other(why),
                BulkError::Io(err) => err,
            }
        }
    }

    /// Read every entry of `dir` with `getattrlistbulk(2)`.
    ///
    /// The per-entry buffer format (documented in getattrlistbulk(2), verified
    /// here against the portable path by tests): each group starts with a
    /// `u32` overall length (8-byte aligned), then an `attribute_set_t` (five
    /// `u32` bitmaps), then each requested attribute in bit order. The name is
    /// an `attrreference_t { i32 offset; u32 length }` whose offset is relative
    /// to the ref's own position.
    pub(super) fn read_bulk(dir: &Path, want_physical: bool) -> Result<Vec<RawEntry>, BulkError> {
        use std::os::unix::ffi::OsStrExt;

        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| BulkError::Fallback("path contains NUL"))?;

        // `open` on a directory with O_RDONLY is valid on macOS.
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::EACCES | libc::EPERM) => {
                    Err(BulkError::Fallback("open unsupported"))
                }
                _ => Err(BulkError::Io(err)),
            };
        }

        let result = read_bulk_fd(fd, want_physical);
        unsafe { libc::close(fd) };
        result
    }

    fn read_bulk_fd(fd: i32, want_physical: bool) -> Result<Vec<RawEntry>, BulkError> {
        let mut attr_list = attrlist {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_DEVID
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_FILEID
                | ATTR_CMN_MODTIME,
            volattr: 0,
            dirattr: 0,
            fileattr: if want_physical {
                ATTR_FILE_DATALENGTH | ATTR_FILE_DATAALLOCSIZE | ATTR_FILE_LINKCOUNT
            } else {
                ATTR_FILE_DATALENGTH | ATTR_FILE_LINKCOUNT
            },
            forkattr: 0,
        };

        // 8-byte aligned backing store; the kernel aligns groups on 8 bytes.
        let mut backing: Vec<u64> = vec![0; CHUNK_BYTES / 8];
        let mut out = Vec::new();

        loop {
            let ret = unsafe {
                libc::getattrlistbulk(
                    fd,
                    &mut attr_list as *mut attrlist as *mut libc::c_void,
                    backing.as_mut_ptr() as *mut libc::c_void,
                    backing.len() * 8,
                    0,
                )
            };
            if ret == 0 {
                break; // no more entries
            }
            if ret < 0 {
                let err = io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::ERANGE) => {
                        // Buffer too small for the chunk; grow and retry.
                        backing.resize(backing.len().saturating_mul(2).max(CHUNK_BYTES / 8), 0);
                        continue;
                    }
                    Some(libc::ENOTSUP | libc::EOPNOTSUPP) => {
                        Err(BulkError::Fallback("getattrlistbulk unsupported"))
                    }
                    _ => Err(BulkError::Io(err)),
                };
            }

            let count = ret as usize;
            let bytes = &backing[..backing.len()];
            let buf = unsafe { std::slice::from_raw_parts(backing.as_ptr() as *const u8, backing.len() * 8) };

            let mut cursor = 0usize;
            for _ in 0..count {
                let (entry, next) = parse_group(buf, cursor)?;
                out.push(entry);
                cursor = next;
            }
            let _ = bytes;
        }

        Ok(out)
    }

    /// A read-only cursor over the chunk buffer with bounds checks, so a
    /// malformed kernel reply degrades to a fallback instead of UB.
    struct Cursor<'a> {
        buf: &'a [u8],
        pos: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(buf: &'a [u8], pos: usize) -> Self {
            Self { buf, pos }
        }

        fn u32(&mut self) -> Option<u32> {
            let bytes = self.buf.get(self.pos..self.pos + 4)?;
            self.pos += 4;
            Some(u32::from_le_bytes(bytes.try_into().unwrap()))
        }

        fn i32(&mut self) -> Option<i32> {
            let bytes = self.buf.get(self.pos..self.pos + 4)?;
            self.pos += 4;
            Some(i32::from_le_bytes(bytes.try_into().unwrap()))
        }

        fn u64(&mut self) -> Option<u64> {
            let bytes = self.buf.get(self.pos..self.pos + 8)?;
            self.pos += 8;
            Some(u64::from_le_bytes(bytes.try_into().unwrap()))
        }

        fn skip(&mut self, n: usize) -> Option<()> {
            self.buf.get(self.pos..self.pos + n)?;
            self.pos += n;
            Some(())
        }
    }

    /// Parse one directory-entry group starting at `pos`; returns the entry and
    /// the byte position of the next group.
    fn parse_group(buf: &[u8], pos: usize) -> Result<(RawEntry, usize), BulkError> {
        let mut cursor = Cursor::new(buf, pos);

        let group_len = cursor
            .u32()
            .ok_or(BulkError::Fallback("truncated group length"))?
            as usize;
        let group_start = pos;
        let group_end = group_start
            .checked_add(group_len)
            .filter(|&end| end <= buf.len())
            .ok_or(BulkError::Fallback("group length out of bounds"))?;

        // attribute_set_t: five u32 bitmaps.
        let returned_common = cursor
            .u32()
            .ok_or(BulkError::Fallback("truncated returned set"))?;
        let _returned_vol = cursor.u32();
        let _returned_dir = cursor.u32();
        let returned_file = cursor.u32().unwrap_or(0);
        let _returned_fork = cursor.u32();

        // Walk the common bitmaps in bit order; RETURNED_ATTRS is first and has
        // no payload, everything else is fixed-size or an attrreference.
        let mut name: Option<Vec<u8>> = None;
        let mut dev: u64 = 0;
        let mut obj_type: u32 = 0;
        let mut file_id: u64 = 0;
        let mut mtime_ms: i64 = 0;

        // Read in bit-value order regardless of request order.
        let mut remaining = returned_common & !ATTR_CMN_RETURNED_ATTRS;
        let mut bit: u32 = 0;
        while remaining != 0 {
            if remaining & (1 << bit) != 0 {
                remaining &= !(1 << bit);
                let attr = 1u32 << bit;
                match attr {
                    ATTR_CMN_NAME => {
                        // attrreference_t { i32 offset; u32 length }
                        let offset = cursor
                            .i32()
                            .ok_or(BulkError::Fallback("truncated name ref"))?;
                        let len = cursor
                            .u32()
                            .ok_or(BulkError::Fallback("truncated name ref"))?;
                        let ref_pos = cursor.pos - 8;
                        let data_start = (ref_pos as i64 + offset as i64) as usize;
                        let end = data_start
                            .checked_add(len as usize)
                            .filter(|&end| end <= buf.len())
                            .ok_or(BulkError::Fallback("name out of bounds"))?;
                        let raw = &buf[data_start..end];
                        let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
                        name = Some(raw.to_vec());
                    }
                    ATTR_CMN_DEVID => {
                        dev = cursor.u32().ok_or(BulkError::Fallback("dev"))? as u64
                    }
                    ATTR_CMN_OBJTYPE => {
                        obj_type = cursor
                            .u32()
                            .ok_or(BulkError::Fallback("objtype"))?
                    }
                    ATTR_CMN_MODTIME => {
                        let secs = cursor
                            .i64_as_u64()
                            .ok_or(BulkError::Fallback("mtime secs"))?;
                        let nsecs = cursor
                            .i64_as_u64()
                            .ok_or(BulkError::Fallback("mtime nsecs"))?;
                        mtime_ms = (secs as i64)
                            .saturating_mul(1000)
                            .saturating_add((nsecs as i64) / 1_000_000);
                    }
                    ATTR_CMN_FILEID => {
                        file_id = cursor
                            .u64()
                            .ok_or(BulkError::Fallback("file id"))?
                    }
                    _ => {
                        // Unknown returned bit: skip a conservative fixed size.
                        cursor.skip(8).ok_or(BulkError::Fallback("unknown attr"))?;
                    }
                }
            }
            bit += 1;
        }
        // File attributes (fixed-size, in bit order).
        let mut logical_size = 0u64;
        let mut physical_size = 0u64;
        let mut nlink = 0u32;
        let mut file_bit: u32 = 0;
        let mut file_remaining = returned_file;
        while file_remaining != 0 {
            if file_remaining & (1 << file_bit) != 0 {
                file_remaining &= !(1 << file_bit);
                let attr = 1u32 << file_bit;
                match attr {
                    ATTR_FILE_DATALENGTH => {
                        logical_size = cursor
                            .u64()
                            .ok_or(BulkError::Fallback("datalength"))?
                    }
                    ATTR_FILE_DATAALLOCSIZE => {
                        physical_size = cursor
                            .u64()
                            .ok_or(BulkError::Fallback("allocsize"))?
                    }
                    ATTR_FILE_LINKCOUNT => {
                        nlink = cursor.u32().ok_or(BulkError::Fallback("linkcount"))?
                    }
                    _ => {
                        cursor.skip(8).ok_or(BulkError::Fallback("unknown file attr"))?;
                    }
                }
            }
            file_bit += 1;
        }

        let next = group_end;
        let name = name.ok_or(BulkError::Fallback("entry without name"))?;

        Ok((
            RawEntry {
                name,
                is_dir: obj_type == VDIR,
                is_symlink: obj_type == VLNK,
                is_file: obj_type == VREG,
                logical_size,
                physical_size,
                mtime_ms,
                dev,
                ino: file_id,
                nlink,
            },
            next,
        ))
    }

    /// Extend the cursor with a signed-8-byte read used for timespec.tv_sec.
    trait CursorExt {
        fn i64_as_u64(&mut self) -> Option<u64>;
    }

    impl<'a> CursorExt for Cursor<'a> {
        fn i64_as_u64(&mut self) -> Option<u64> {
            let bytes = self.buf.get(self.pos..self.pos + 8)?;
            self.pos += 8;
            Some(u64::from_le_bytes(bytes.try_into().unwrap()))
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Build a small tree with a symlink, subdirectory, and files of known
    /// sizes, then assert the fast path and the portable path agree on every
    /// field that both report.
    #[test]
    fn fast_path_matches_portable_on_real_tree() {
        let base = std::env::temp_dir().join(format!("sift-dirread-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("sub/deep")).unwrap();
        fs::write(base.join("a.txt"), vec![b'x'; 1234]).unwrap();
        fs::write(base.join("b.bin"), vec![b'y'; 65536]).unwrap();
        fs::write(base.join("sub/c.txt"), b"hello").unwrap();
        fs::write(base.join("sub/deep/d.txt"), b"deep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("a.txt"), base.join("link")).unwrap();

        let fast = DirReader::read(&base, true).expect("fast path must work on a real dir");
        let portable = DirReader::read_portable(&base, true).expect("portable path");

        fn canon(entries: &[RawEntry]) -> BTreeMap<Vec<u8>, RawEntry> {
            entries
                .iter()
                .map(|e| (e.name.clone(), e.clone()))
                .collect()
        }
        let fast_map = canon(fast.entries());
        let portable_map = canon(portable.entries());

        assert_eq!(
            fast_map.keys().collect::<Vec<_>>(),
            portable_map.keys().collect::<Vec<_>>(),
            "fast and portable disagree on entry names"
        );
        for (name, fast_entry) in &fast_map {
            let port = &portable_map[name];
            assert_eq!(fast_entry.is_dir, port.is_dir, "{name:?} is_dir");
            assert_eq!(fast_entry.is_symlink, port.is_symlink, "{name:?} is_symlink");
            // A directory's st_size is filesystem-defined and meaningless (the
            // engine never aggregates it); only files must agree on bytes.
            if !fast_entry.is_dir {
                assert_eq!(fast_entry.logical_size, port.logical_size, "{name:?} size");
            }
            if port.mtime_ms != 0 && fast_entry.mtime_ms != 0 {
                // Both report unix-ms mtimes; allow sub-second rounding.
                assert!(
                    (fast_entry.mtime_ms - port.mtime_ms).abs() <= 1000,
                    "{name:?} mtime {} vs {}",
                    fast_entry.mtime_ms,
                    port.mtime_ms
                );
            }
            if fast_entry.dev != 0 {
                assert_eq!(fast_entry.dev, port.dev, "{name:?} dev");
            }
            if fast_entry.ino != 0 {
                assert_eq!(fast_entry.ino, port.ino, "{name:?} ino");
            }
        }

        // Directories report a size; files report their exact bytes.
        let a = &fast_map[&b"a.txt".to_vec()];
        assert_eq!(a.logical_size, 1234);
        assert!(a.physical_size > 0, "allocated bytes should be reported");
        let sub = &fast_map[&b"sub".to_vec()];
        assert!(sub.is_dir);
        assert_eq!(sub.logical_size, 0, "directory st_size is 0 on APFS");

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn link_counts_are_reported_so_dedupe_can_be_selective() {
        let base = std::env::temp_dir().join(format!("sift-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let original = base.join("original.bin");
        fs::write(&original, b"payload").unwrap();
        let linked = base.join("linked.bin");
        fs::hard_link(&original, &linked).unwrap();

        let fast = DirReader::read(&base, true).expect("fast path");
        let portable = DirReader::read_portable(&base, true).expect("portable path");
        for reader in [&fast, &portable] {
            let by_name = |name: &str| {
                reader
                    .entries()
                    .iter()
                    .find(|entry| entry.name == name.as_bytes())
                    .unwrap_or_else(|| panic!("{name} missing"))
            };
            assert_eq!(by_name("original.bin").nlink, 2, "hardlinked file");
            assert_eq!(by_name("linked.bin").nlink, 2, "hardlinked file");
            // Both names share one inode, which is what lets the tree count the
            // bytes once.
            assert_eq!(
                by_name("original.bin").file_id(),
                by_name("linked.bin").file_id()
            );
        }

        let single = base.join("single.bin");
        fs::write(&single, b"alone").unwrap();
        let reader = DirReader::read(&base, true).unwrap();
        let entry = reader
            .entries()
            .iter()
            .find(|entry| entry.name == b"single.bin")
            .unwrap();
        assert_eq!(entry.nlink, 1, "a single-linked file must not enter dedupe");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn portable_reads_even_without_permissions_edges() {
        // At minimum the empty-dir case must work on both paths.
        let base = std::env::temp_dir().join(format!("sift-dirread-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let fast = DirReader::read(&base, true).unwrap();
        assert!(fast.entries().is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn entry_identity_flags() {
        let e = RawEntry {
            name: b"x".to_vec(),
            is_dir: false,
            is_symlink: false,
            is_file: true,
            logical_size: 10,
            physical_size: 4096,
            mtime_ms: 0,
            dev: 1,
            ino: 0,
            nlink: 1,
        };
        assert!(e.file_id().is_none(), "ino 0 must not dedupe");
        let e2 = RawEntry {
            ino: 42,
            ..e
        };
        assert_eq!(e2.file_id(), Some(FileId::new(1, 42)));
        assert_eq!(e2.size().logical, 10);
        assert_eq!(e2.size().physical, 4096);
    }
}
