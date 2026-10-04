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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use sift_core::id::FileId;

#[cfg(target_os = "macos")]
pub mod fd {
    //! File-descriptor helpers for an `openat`-based walker.
    //!
    //! Opening a child relative to its parent's descriptor avoids re-resolving
    //! the whole deep path on every directory (measured 2–3× faster than
    //! opening by absolute path).
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::RawFd;
    use std::path::Path;

    /// Open a directory by absolute path. On success the descriptor consumes
    /// one walker-budget slot, released when it is closed with
    /// [`super::close_raw_fd`].
    pub fn open_path(path: &Path) -> io::Result<RawFd> {
        let c_path = CString::new(path.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        super::fd_budget::acquire();
        let fd = unsafe {
            libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY)
        };
        if fd < 0 {
            #[cfg(target_os = "macos")]
            super::fd_budget::release();
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    /// Open a child directory named `name` relative to `parent`.
    pub fn open_at(parent: RawFd, name: &[u8]) -> io::Result<RawFd> {
        let c_name = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                parent,
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    /// Close a descriptor.
    pub fn close(fd: RawFd) -> io::Result<()> {
        if unsafe { libc::close(fd) } != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Why a directory could not be read. Coarse on purpose: the scan engine only
/// needs to tell "the user can grant this" apart from real failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadErrorKind {
    /// The directory vanished or never existed.
    NotFound,
    /// Access denied by the macOS TCC privacy subsystem even though this
    /// process satisfies the POSIX owner/mode bits. A user selection in an
    /// open panel grants persistent access.
    PermissionTcc,
    /// Access denied by POSIX ownership/mode bits themselves (typically a
    /// root-owned 0700 system directory). A panel user selection cannot
    /// grant this; only an administrator can.
    PermissionPosix,
    /// Any other I/O error.
    Other,
}

/// Map an OS error to a [`ReadErrorKind`]. Permission errors are classified
/// against `path` (see [`classify_denial`]).
fn classify_io_error_at(err: &io::Error, path: &Path) -> ReadErrorKind {
    #[cfg(unix)]
    {
        match err.raw_os_error() {
            Some(libc::ENOENT) => return ReadErrorKind::NotFound,
            Some(libc::EACCES | libc::EPERM) => return classify_denial(path),
            _ => {}
        }
    }
    match err.kind() {
        io::ErrorKind::NotFound => ReadErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => ReadErrorKind::PermissionTcc,
        _ => ReadErrorKind::Other,
    }
}

/// Classify an EACCES/EPERM on `path` as TCC or POSIX. The POSIX decision is
/// reconstructed from the directory's stat (stat itself is not gated by TCC):
/// when this process would pass the owner/group/other check yet `open` failed,
/// the denial comes from TCC; otherwise the mode bits keep us out.
fn classify_denial(path: &Path) -> ReadErrorKind {
    #[cfg(unix)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode();
            let posix_allows = if unsafe { libc::geteuid() } == meta.uid() {
                mode & 0o500 == 0o500
            } else {
                let mut groups = [0u32; 64];
                let count = unsafe {
                    libc::getgroups(groups.len() as i32, groups.as_mut_ptr())
                };
                let in_group = count > 0
                    && groups[..count as usize].contains(&meta.gid());
                if in_group {
                    mode & 0o050 == 0o050
                } else {
                    mode & 0o005 == 0o005
                }
            };
            return if posix_allows {
                ReadErrorKind::PermissionTcc
            } else {
                ReadErrorKind::PermissionPosix
            };
        }
    }
    let _ = path;
    // Stat failed: optimistically offer the grant flow rather than blocking it.
    ReadErrorKind::PermissionTcc
}

/// Process-wide budget on descriptors held by the walker: preopened child
/// descriptors riding the scan queue plus one transient descriptor per
/// reading worker.
///
/// Without the cap, a directory containing thousands of subdirectories (nested
/// package caches, build trees) would push the process to EMFILE, and
/// cancelling a scan would leak every descriptor still queued. The cap means
/// children beyond the budget simply are not preopened and fall back to a
/// path open when dispatched.
#[cfg(target_os = "macos")]
mod fd_budget {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;

    static HELD: AtomicUsize = AtomicUsize::new(0);
    static LIMIT: OnceLock<usize> = OnceLock::new();
    /// Subset of HELD: preopened descriptors riding the priority queue,
    /// waiting to be dispatched.
    static QUEUED: AtomicUsize = AtomicUsize::new(0);

    /// Descriptors reserved for everything but the walker (stdio, loaded
    /// libraries, IPC, the SQLite journal...).
    const RESERVE: usize = 64;
    /// Absolute ceiling even when `rlimit` allows far more: keeps the set of
    /// fds riding the queue small and predictable.
    const MAX_HELD: usize = 448;
    /// Preferred open-file table size; macOS' default soft limit is 256.
    const PREFERRED_NOFILE: u64 = 1024;

    /// Number of descriptors the walker may hold at once.
    pub fn limit() -> usize {
        *LIMIT.get_or_init(|| {
            raise_nofile();
            let soft = current_nofile();
            soft.saturating_sub(RESERVE).clamp(1, MAX_HELD)
        })
    }

    /// Try to claim one descriptor slot without blocking.
    pub fn try_acquire() -> bool {
        let cap = limit();
        let mut current = HELD.load(Ordering::Relaxed);
        loop {
            if current >= cap {
                return false;
            }
            match HELD.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Claim a slot for a transient (path-opened) directory descriptor,
    /// briefly waiting for one to free up. If the budget stays exhausted —
    /// only possible under pathological scheduling — the count is allowed to
    /// oversubscribe by one so a worker can never deadlock: every close is
    /// symmetric, so the counter returns under the cap as the queue drains.
    pub fn acquire() {
        if try_acquire() {
            return;
        }
        for _ in 0..256 {
            std::thread::yield_now();
            if try_acquire() {
                return;
            }
        }
        for _ in 0..16 {
            std::thread::sleep(std::time::Duration::from_micros(100));
            if try_acquire() {
                return;
            }
        }
        HELD.fetch_add(1, Ordering::AcqRel);
    }

    /// Return one slot after a descriptor is closed.
    pub fn release() {
        let previous = HELD.fetch_sub(1, Ordering::Release);
        debug_assert!(previous > 0, "fd budget released without an acquire");
    }

    /// Cap on descriptors queued ahead of dispatch. Kept far below the main
    /// cap so a backlog of preopened jobs cannot grab every slot and then sit
    /// behind higher-priority path-open jobs (which would sleep in `acquire`
    /// while the fds ride jobs that never pop — observed as a minutes-long,
    /// CPU-burning stall with zero UI progress).
    const QUEUED_CAP: usize = 48;

    /// Claim a slot for a preopened descriptor that will ride the queue.
    pub fn try_acquire_queued() -> bool {
        if QUEUED.load(Ordering::Relaxed) >= QUEUED_CAP {
            return false;
        }
        if !try_acquire() {
            return false;
        }
        QUEUED.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// A queued descriptor is being dispatched to a worker: it becomes an
    /// ordinary transient slot. HELD is untouched; only the queued subset
    /// drops.
    pub fn adopt_queued() {
        let previous = QUEUED.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "queued fd adopted without a queued slot");
    }

    /// Release a queued descriptor that was closed before dispatch (a job
    /// dropped during cancellation, an un-adopted child).
    pub fn release_queued() {
        let previous = QUEUED.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "queued fd released without a queued slot");
        release();
    }

    /// Walker-held slot count, for tests asserting acquire/release symmetry.
    #[cfg(test)]
    pub fn held() -> usize {
        HELD.load(Ordering::Relaxed)
    }

    fn current_nofile() -> usize {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return 0;
        }
        let limit = unsafe { limit.assume_init() };
        if limit.rlim_cur == libc::RLIM_INFINITY {
            usize::MAX
        } else {
            limit.rlim_cur as usize
        }
    }

    /// Raise the soft `RLIMIT_NOFILE` toward [`PREFERRED_NOFILE`], bounded by
    /// the hard limit. Best effort: a failure leaves the default in place and
    /// the budget shrinks accordingly.
    fn raise_nofile() {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return;
        }
        let mut limit = unsafe { limit.assume_init() };
        if limit.rlim_cur >= PREFERRED_NOFILE {
            return;
        }
        let target = if limit.rlim_max == libc::RLIM_INFINITY {
            PREFERRED_NOFILE
        } else {
            limit.rlim_max.min(PREFERRED_NOFILE)
        };
        if target > limit.rlim_cur {
            limit.rlim_cur = target;
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
        }
    }
}

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
    /// Permission bits (`st_mode & 0o7777`). `0` when not reported.
    pub mode: u32,
    /// Owning uid. `u32::MAX` when not reported.
    pub uid: u32,
    /// Owning gid. `u32::MAX` when not reported.
    pub gid: u32,
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

    /// Whether the current process could unlink this object's children, i.e.
    /// whether the directory is writable for us. Computed from the entry's own
    /// permission bits as returned inside the parent's bulk result, which lets
    /// the walker avoid an `fstat` per directory.
    #[cfg(unix)]
    pub fn writable_by_us(&self) -> bool {
        if self.mode == 0 {
            return false;
        }
        let write_bit = if self.uid != u32::MAX && self.uid == unsafe { libc::geteuid() } {
            0o200
        } else if self.gid != u32::MAX && self.gid == unsafe { libc::getegid() } {
            0o020
        } else {
            0o002
        };
        self.mode & write_bit != 0
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

/// Per-worker reusable backing store for bulk enumeration.
///
/// On macOS this is the 8-byte-aligned buffer handed to
/// `getattrlistbulk(2)`; it grows on `ERANGE` and keeps that capacity for the
/// remaining millions of directories the worker will read. On other platforms
/// it is an empty marker kept so callers stay platform-independent.
#[derive(Debug, Default)]
pub struct BulkBuffer {
    #[cfg(target_os = "macos")]
    inner: Vec<u64>,
}

impl BulkBuffer {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Descriptors of a directory's children, opened with `openat` while the
/// parent descriptor was still live, keyed by the child's name bytes.
///
/// The coordinator takes the ones it is going to dispatch and attaches them
/// to the child jobs; any it declines (mount boundaries, adopted-closed
/// subtrees, budget-refused size-only children) must be closed with
/// [`ChildFdMap::close_remaining`] so descriptors never leak.
/// Descriptors of a directory's children, opened with `openat` while the
/// parent descriptor was still live.
///
/// Stored as raw pairs (no per-result `HashMap` churn: nearly every one of the
/// ~1M directories processed has only a handful of children). Lookup is
/// linear while the list is small; a map is built lazily the first time a
/// large directory is queried. Declined descriptors are closed with
/// [`ChildFdMap::close_remaining`].
pub struct ChildFdMap {
    pairs: Vec<(Vec<u8>, i32)>,
    built: Option<HashMap<Vec<u8>, i32>>,
}

impl Default for ChildFdMap {
    fn default() -> Self {
        Self {
            pairs: Vec::new(),
            built: None,
        }
    }
}

impl core::fmt::Debug for ChildFdMap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChildFdMap").field("len", &self.pairs.len()).finish()
    }
}

/// Directories up to this size are looked up by linear scan; beyond it the
/// map is built once.
const LINEAR_CHILD_LOOKUP: usize = 24;

impl ChildFdMap {
    pub fn from_pairs(pairs: Vec<(Vec<u8>, i32)>) -> Self {
        Self {
            pairs,
            built: None,
        }
    }

    /// Take the preopened descriptor for `name`, if present.
    pub fn take(&mut self, name: &[u8]) -> Option<i32> {
        if self.built.is_some() || self.pairs.len() > LINEAR_CHILD_LOOKUP {
            let map = self
                .built
                .get_or_insert_with(|| self.pairs.drain(..).collect());
            return map.remove(name);
        }
        self.pairs
            .iter()
            .position(|(child_name, _)| child_name == name)
            .map(|pos| self.pairs.remove(pos).1)
    }

    /// Close every descriptor still held, once the coordinator has picked the
    /// children it wants.
    pub fn close_remaining(&mut self) {
        // Map-held descriptors still carry their queued classification:
        // release both counters on macOS.
        #[cfg(target_os = "macos")]
        {
            if let Some(map) = self.built.as_mut() {
                for (_, fd) in map.drain() {
                    close_queued_fd(fd);
                }
                return;
            }
            for (_, fd) in self.pairs.drain(..) {
                close_queued_fd(fd);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(map) = self.built.as_mut() {
                for (_, fd) in map.drain() {
                    close_raw_fd(fd);
                }
                return;
            }
            for (_, fd) in self.pairs.drain(..) {
                close_raw_fd(fd);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.built.as_ref().map_or(self.pairs.len(), HashMap::len)
    }
}

/// Safety net: descriptors left in a map when it is dropped (an early return,
/// a cancelled scan) are closed instead of leaking.
#[cfg(unix)]
impl Drop for ChildFdMap {
    fn drop(&mut self) {
        self.close_remaining();
    }
}

/// Close a raw descriptor. Small unix-only seam so callers do not need their
/// own `libc` dependency for descriptor cleanup.
#[cfg(unix)]
pub fn close_raw_fd(fd: std::os::unix::io::RawFd) {
    unsafe { libc::close(fd) };
    // Every descriptor the walker closes came from a budgeted open
    // (`open_path` / `open_child_dirs`); release its slot.
    #[cfg(target_os = "macos")]
    fd_budget::release();
}

/// Close a preopened descriptor that never left the queue (a job dropped
/// during cancellation) and release its queued slot.
#[cfg(target_os = "macos")]
pub fn close_queued_fd(fd: std::os::unix::io::RawFd) {
    unsafe { libc::close(fd) };
    fd_budget::release_queued();
}

/// The descriptor riding `job` is dispatched and becomes transient; drop its
/// queued classification. No-op off macOS.
#[cfg(target_os = "macos")]
pub fn adopt_queued_fd() {
    fd_budget::adopt_queued();
}

/// See [`adopt_queued_fd`].
#[cfg(not(target_os = "macos"))]
pub fn adopt_queued_fd() {}

/// `st_size` of an already-open descriptor, via `fstat(2)`.
///
/// Used to spot a giant directory before enumerating it: on APFS the
/// directory's size tracks its entry count. The descriptor must stay open;
/// this is not a destructive probe.
#[cfg(unix)]
pub fn fd_size(fd: std::os::unix::io::RawFd) -> Option<u64> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == 0 {
        Some(unsafe { stat.assume_init() }.st_size as u64)
    } else {
        None
    }
}

/// `st_size` via a path-stat, when no descriptor is available (the scan root
/// or a portable-fallback walk).
pub fn path_size(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|meta| meta.len())
}

/// Open every real subdirectory child with `openat` relative to `parent`,
/// returning `(name, fd)` pairs. Symlinks and `.`/`..` are skipped. Done
/// while the parent descriptor is open, so no path resolution is needed.
#[cfg(target_os = "macos")]
pub fn open_child_dirs(
    parent: std::os::unix::io::RawFd,
    entries: &[RawEntry],
) -> Vec<(Vec<u8>, std::os::unix::io::RawFd)> {
    let mut pairs = Vec::new();
    for entry in entries {
        if entry.is_dir
            && !entry.is_symlink
            && entry.name != b"."
            && entry.name != b".."
        {
            // Claim a queued slot before opening; when the budget (or the
            // smaller queue cap) is exhausted skip the preopen — the child
            // job opens by path later.
            if !fd_budget::try_acquire_queued() {
                continue;
            }
            if let Ok(name) = std::ffi::CString::new(entry.name.as_slice()) {
                let fd = unsafe {
                    libc::openat(
                        parent,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY,
                    )
                };
                if fd >= 0 {
                    pairs.push((entry.name.clone(), fd));
                } else {
                    // Open failed (TCC denial, vanished entry): give the slot
                    // back rather than closing an acquired descriptor.
                    fd_budget::release_queued();
                }
            } else {
                fd_budget::release_queued();
            }
        }
    }
    pairs
}

#[cfg(not(target_os = "macos"))]
pub fn open_child_dirs(
    _parent: i32,
    _entries: &[RawEntry],
) -> Vec<(Vec<u8>, i32)> {
    Vec::new()
}

/// What the scan engine needs from one directory: the entries it reported and
/// preopened descriptors for every real subdirectory, so the child jobs can
/// skip path resolution entirely.
pub struct ReadOutcome {
    /// Entries of the directory, or why it could not be read. The error is
    /// classified so the engine can wait for authorization instead of
    /// treating every failure alike.
    pub entries: Result<Vec<RawEntry>, ReadErrorKind>,
    pub child_fds: ChildFdMap,
}

/// Read one directory and preopen its subdirectories.
///
/// `preopened` is the directory's descriptor when the worker inherited it
/// from the parent's result; the descriptor is consumed (closed) here. When
/// `None`, the directory is opened from `path` once. Subdirectories are
/// opened with `openat` before the parent closes, so no path is resolved.
/// On platforms without fd support this degrades to the portable reader and
/// returns no child descriptors.
pub fn read_with_children(
    preopened: Option<i32>,
    path: &Path,
    want_physical: bool,
    buffer: &mut BulkBuffer,
) -> ReadOutcome {
    #[cfg(target_os = "macos")]
    {
        let opened = match preopened {
            Some(fd) => Ok(fd),
            None => fd::open_path(path),
        };
        let dir_fd = match opened {
            Ok(fd) => fd,
            Err(err) => {
                return ReadOutcome {
                    entries: Err(classify_io_error_at(&err, path)),
                    child_fds: ChildFdMap::default(),
                };
            }
        };
        let entries = DirReader::read_fd_reusing(dir_fd, want_physical, buffer);
        // Preopen children even if the bulk read failed; the entries list is
        // simply empty in that case.
        let pairs =
            open_child_dirs(dir_fd, entries.as_ref().map(Vec::as_slice).unwrap_or(&[]));
        close_raw_fd(dir_fd);
        ReadOutcome {
            entries: entries.map_err(|err| classify_io_error_at(&err, path)),
            child_fds: ChildFdMap::from_pairs(pairs),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        debug_assert!(preopened.is_none(), "fd support exists only on macOS");
        let _ = buffer;
        let entries = DirReader::read_portable(path, want_physical)
            .map(|reader| reader.into_parts().1)
            .map_err(|err| classify_io_error_at(&err, path));
        ReadOutcome {
            entries,
            child_fds: ChildFdMap::default(),
        }
    }
}

/// Diagnostic probe: cumulative nanoseconds spent opening directories vs
/// running the bulk loop. Read with [`probe_timings`].
static OPEN_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static BULK_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn probe_timings() -> (u64, u64) {
    use std::sync::atomic::Ordering;
    (OPEN_NS.load(Ordering::Relaxed), BULK_NS.load(Ordering::Relaxed))
}

/// The result of reading one directory.
#[derive(Debug)]
#[non_exhaustive]
pub struct DirReader {
    kind: ReaderKind,
    /// Whether the read directory itself is writable, i.e. its entries may be
    /// unlinked. Computed from the already-open descriptor, so it costs no
    /// extra path lookup.
    writable: bool,
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
                Ok((writable, entries)) => {
                    return Ok(Self {
                        kind: ReaderKind::GetAttrListBulk,
                        writable,
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

    /// Like [`Self::read`] but reuses a caller-owned bulk buffer across
    /// directories. A worker that walks millions of directories otherwise
    /// allocates and zeroes the initial 16 KiB buffer for every one of them;
    /// retaining the capacity per worker removes that churn.
    ///
    /// Returns only the entries: the caller already knows the directory's
    /// writability from the mode bits in its parent's bulk result, so this
    /// path performs no extra `fstat` (one fewer locked-kernel call per
    /// directory — millions across a volume).
    pub fn read_reusing(
        dir: &Path,
        want_physical: bool,
        buffer: &mut BulkBuffer,
    ) -> io::Result<Vec<RawEntry>> {
        #[cfg(target_os = "macos")]
        {
            match macos::read_bulk_reusing(dir, want_physical, &mut buffer.inner) {
                Ok(entries) => return Ok(entries),
                Err(err) => {
                    if !err.is_fallback() {
                        return Err(err.into_io());
                    }
                    // Unsupported or unreadable via the bulk path: fall through.
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = buffer;
        }
        Ok(Self::read_portable(dir, want_physical)?.into_parts().1)
    }

    /// One-shot writability probe for a scan root, which has no parent bulk
    /// result to inherit mode bits from. Everything below the root gets its
    /// writability from its parent's entries, so a full scan performs this
    /// exactly once.
    #[cfg(unix)]
    pub fn probe_writable(dir: &Path) -> bool {
        writable_of_path(dir)
    }

    /// Read one directory already open as `fd`, without a path lookup.
    ///
    /// Exposed so a walker can open children with `openat(parent_fd, name)` and
    /// avoid re-resolving deep paths. The descriptor is not consumed: the caller
    /// closes it. Returns the portable metadata kind so callers cannot tell how
    /// the read was produced beyond [`DirReader::kind`].
    pub fn read_fd(fd: std::os::unix::io::RawFd, want_physical: bool) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let writable = writable_of_raw_fd(fd);
            match macos::read_bulk_fd(fd, want_physical) {
                Ok(entries) => {
                    return Ok(Self {
                        kind: ReaderKind::GetAttrListBulk,
                        writable,
                        entries,
                    });
                }
                Err(err) => {
                    if !err.is_fallback() {
                        return Err(err.into_io());
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        {
            let _ = want_physical;
            Err(io::Error::other("fd reading unsupported on this platform"))
        }
    }

    /// Read an already-open directory, reusing the worker's bulk buffer.
    ///
    /// Returns the entries directly so a worker holding a parent descriptor
    /// can enumerate it without any path lookup, then open the children with
    /// `openat` relative to this same descriptor.
    pub fn read_fd_reusing(
        fd: std::os::unix::io::RawFd,
        want_physical: bool,
        buffer: &mut BulkBuffer,
    ) -> io::Result<Vec<RawEntry>> {
        #[cfg(target_os = "macos")]
        {
            macos::read_bulk_fd_reusing(fd, want_physical, &mut buffer.inner)
                .map_err(|err| err.into_io())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (want_physical, buffer);
            Err(io::Error::other("fd reading unsupported on this platform"))
        }
    }

    /// The portable implementation, exposed so tests and benchmarks can compare
    /// it against the fast path on identical input.
    pub fn read_portable(dir: &Path, want_physical: bool) -> io::Result<Self> {
        let mut entries = Vec::new();
        let dir_stream = fs::read_dir(dir)?;
        let writable = writable_of_path(dir);
        for entry in dir_stream {
            let entry = entry?;
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(_) => continue, // vanished between readdir and stat
            };
            let file_type = meta.file_type();
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
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
                    mode: meta.mode() & 0o7777,
                    uid: meta.uid(),
                    gid: meta.gid(),
                });
            }
            #[cfg(not(unix))]
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
                mode: 0,
                uid: u32::MAX,
                gid: u32::MAX,
            });
        }
        Ok(Self {
            kind: ReaderKind::Portable,
            writable,
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

    /// Whether the directory is writable (its entries can be unlinked).
    #[inline]
    pub fn writable(&self) -> bool {
        self.writable
    }

    #[inline]
    pub fn into_parts(self) -> (bool, Vec<RawEntry>) {
        (self.writable, self.entries)
    }
}

/// Portable permission probe: stat the directory itself (the object whose
/// write bit governs unlinking its children) and test the matching write bit.
#[cfg(unix)]
fn writable_of_path(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let write_bit = if meta.uid() == unsafe { libc::geteuid() } {
        0o200
    } else if meta.gid() == unsafe { libc::getegid() } {
        0o020
    } else {
        0o002
    };
    meta.mode() & write_bit != 0
}

/// Same test as [`writable_of_fd`] for a raw descriptor that is already open
/// (as in the macOS bulk path, which opens the directory itself).
#[cfg(unix)]
fn writable_of_raw_fd(fd: std::os::unix::io::RawFd) -> bool {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return false;
    }
    let stat = unsafe { stat.assume_init() };
    let write_bit = if stat.st_uid == unsafe { libc::geteuid() } {
        0o200
    } else if stat.st_gid == unsafe { libc::getegid() } {
        0o020
    } else {
        0o002
    };
    stat.st_mode & write_bit != 0
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
    meta.mtime()
        .saturating_mul(1000)
        .saturating_add(meta.mtime_nsec() / 1_000_000)
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
        attrlist, ATTR_BIT_MAP_COUNT, ATTR_CMN_DEVID, ATTR_CMN_FILEID, ATTR_CMN_MODTIME,
        ATTR_CMN_ACCESSMASK, ATTR_CMN_GRPID, ATTR_CMN_NAME, ATTR_CMN_OBJTYPE,
        ATTR_CMN_OWNERID, ATTR_CMN_RETURNED_ATTRS, ATTR_FILE_DATAALLOCSIZE,
        ATTR_FILE_DATALENGTH, ATTR_FILE_LINKCOUNT,
    };

    /// Values of `enum vtype` from `<sys/vnode.h>`:
    /// VNON=0, VREG=1, VDIR=2, VBLK=3, VCHR=4, VLNK=5.
    const VREG: u32 = 0x1;
    const VDIR: u32 = 0x2;
    const VLNK: u32 = 0x5;

    /// macOS returns entries in groups; this is the max we buffer in one call.
    const CHUNK_BYTES: usize = 1024 * 1024;
    /// Ceiling for a worker's reusable bulk buffer, so ERANGE growth stays
    /// bounded (16 MiB per worker worst case).
    const MAX_BACKING_BYTES: usize = 16 * 1024 * 1024;
    /// Most directories hold a handful of entries, so start small and grow only
    /// when a listing asks for more — avoids zeroing a megabyte per directory.
    const INITIAL_BYTES: usize = 16 * 1024;

    /// Benchmark probe: override the initial bulk buffer size via
    /// `SIFT_BULK_KB`, and pass extra `getattrlistbulk` option bits via
    /// `SIFT_BULK_OPTS` (bitmask, e.g. 2 = FSOPT_NOINMEMUPDATE).
    fn tuning() -> (usize, u64) {
        use std::sync::OnceLock;
        static TUNING: OnceLock<(usize, u64)> = OnceLock::new();
        *TUNING.get_or_init(|| {
            let initial = std::env::var("SIFT_BULK_KB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .map(|kb| kb * 1024)
                .unwrap_or(INITIAL_BYTES);
            let opts = std::env::var("SIFT_BULK_OPTS")
                .ok()
                .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            (initial, opts)
        })
    }

    /// Benchmark probe: `SIFT_MIN_ATTRS=1` requests only name/type/data-length
    /// to measure the per-attribute kernel cost of bulk enumeration.
    fn min_attrs() -> bool {
        use std::sync::OnceLock;
        static MIN: OnceLock<bool> = OnceLock::new();
        *MIN.get_or_init(|| std::env::var("SIFT_MIN_ATTRS").as_deref() == Ok("1"))
    }

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
    pub(super) fn read_bulk(
        dir: &Path,
        want_physical: bool,
    ) -> Result<(bool, Vec<RawEntry>), BulkError> {
        let mut backing = vec![0u64; INITIAL_BYTES / 8];
        let entries = read_bulk_reusing(dir, want_physical, &mut backing)?;
        Ok((super::writable_of_path(dir), entries))
    }

    /// Same as [`read_bulk`], but the 8-byte-aligned kernel buffer is supplied
    /// by the caller and retains its grown capacity across directories.
    pub(super) fn read_bulk_reusing(
        dir: &Path,
        want_physical: bool,
        backing: &mut Vec<u64>,
    ) -> Result<Vec<RawEntry>, BulkError> {
        use std::os::unix::ffi::OsStrExt;

        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| BulkError::Fallback("path contains NUL"))?;

        // `open` on a directory with O_RDONLY is valid on macOS.
        let open_start = std::time::Instant::now();
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
        OPEN_NS.fetch_add(
            open_start.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        let bulk_start = std::time::Instant::now();
        let result = read_bulk_fd_reusing(fd, want_physical, backing);
        BULK_NS.fetch_add(
            bulk_start.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        unsafe { libc::close(fd) };
        result
    }

    pub(super) fn read_bulk_fd(
        fd: i32,
        want_physical: bool,
    ) -> Result<Vec<RawEntry>, BulkError> {
        let mut backing = vec![0u64; INITIAL_BYTES / 8];
        read_bulk_fd_reusing(fd, want_physical, &mut backing)
    }

    /// Core bulk loop with a caller-owned, reusable kernel buffer.
    pub(super) fn read_bulk_fd_reusing(
        fd: i32,
        want_physical: bool,
        backing: &mut Vec<u64>,
    ) -> Result<Vec<RawEntry>, BulkError> {
        let (initial_bytes, extra_opts) = tuning();
        let initial_words = initial_bytes / 8;
        if backing.capacity() < initial_words {
            backing.reserve(initial_words - backing.len());
        }
        // Keep the grown allocation but reset to the small working length.
        backing.resize(initial_words, 0);

        let mut attr_list = attrlist {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: if min_attrs() {
                ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE
            } else {
                ATTR_CMN_RETURNED_ATTRS
                    | ATTR_CMN_NAME
                    | ATTR_CMN_DEVID
                    | ATTR_CMN_OBJTYPE
                    | ATTR_CMN_FILEID
                    | ATTR_CMN_MODTIME
                    | ATTR_CMN_ACCESSMASK
                    | ATTR_CMN_OWNERID
                    | ATTR_CMN_GRPID
            },
            volattr: 0,
            dirattr: 0,
            fileattr: if min_attrs() {
                ATTR_FILE_DATALENGTH
            } else if want_physical {
                ATTR_FILE_DATALENGTH | ATTR_FILE_DATAALLOCSIZE | ATTR_FILE_LINKCOUNT
            } else {
                ATTR_FILE_DATALENGTH | ATTR_FILE_LINKCOUNT
            },
            forkattr: 0,
        };

        let mut out = Vec::new();

        loop {
            let ret = unsafe {
                libc::getattrlistbulk(
                    fd,
                    &mut attr_list as *mut attrlist as *mut libc::c_void,
                    backing.as_mut_ptr() as *mut libc::c_void,
                    backing.len() * 8,
                    extra_opts as libc::c_ulong,
                )
            };
            if ret == 0 {
                break; // no more entries
            }
            if ret < 0 {
                let err = io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::ERANGE) => {
                        // Buffer too small for the chunk; grow and retry, but
                        // stop at MAX_BACKING_BYTES so one pathological entry
                        // cannot make a worker's buffer grow without bound.
                        if backing.len() * 8 >= MAX_BACKING_BYTES {
                            return Err(BulkError::Fallback(
                                "entry exceeds bulk buffer cap",
                            ));
                        }
                        backing.resize(
                            backing.len().saturating_mul(2).max(CHUNK_BYTES / 8),
                            0,
                        );
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
            let buf = unsafe {
                std::slice::from_raw_parts(backing.as_ptr() as *const u8, backing.len() * 8)
            };

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
            .ok_or(BulkError::Fallback("truncated group length"))? as usize;
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
        let mut mode: u32 = 0;
        let mut uid: u32 = u32::MAX;
        let mut gid: u32 = u32::MAX;

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
                    ATTR_CMN_DEVID => dev = cursor.u32().ok_or(BulkError::Fallback("dev"))? as u64,
                    ATTR_CMN_OBJTYPE => {
                        obj_type = cursor.u32().ok_or(BulkError::Fallback("objtype"))?
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
                        file_id = cursor.u64().ok_or(BulkError::Fallback("file id"))?
                    }
                    ATTR_CMN_OWNERID => {
                        uid = cursor.u32().ok_or(BulkError::Fallback("owner id"))?
                    }
                    ATTR_CMN_GRPID => {
                        gid = cursor.u32().ok_or(BulkError::Fallback("group id"))?
                    }
                    ATTR_CMN_ACCESSMASK => {
                        mode = cursor.u32().ok_or(BulkError::Fallback("access mask"))?
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
                        logical_size = cursor.u64().ok_or(BulkError::Fallback("datalength"))?
                    }
                    ATTR_FILE_DATAALLOCSIZE => {
                        physical_size = cursor.u64().ok_or(BulkError::Fallback("allocsize"))?
                    }
                    ATTR_FILE_LINKCOUNT => {
                        nlink = cursor.u32().ok_or(BulkError::Fallback("linkcount"))?
                    }
                    _ => {
                        cursor
                            .skip(8)
                            .ok_or(BulkError::Fallback("unknown file attr"))?;
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
                mode,
                uid,
                gid,
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
            assert_eq!(
                fast_entry.is_symlink, port.is_symlink,
                "{name:?} is_symlink"
            );
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
            mode: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        assert!(e.file_id().is_none(), "ino 0 must not dedupe");
        let e2 = RawEntry { ino: 42, ..e };
        assert_eq!(e2.file_id(), Some(FileId::new(1, 42)));
        assert_eq!(e2.size().logical, 10);
        assert_eq!(e2.size().physical, 4096);
    }

    #[cfg(unix)]
    #[test]
    fn denial_is_classified_tcc_vs_posix() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir()
            .join(format!("sift-deny-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tcc_like = base.join("openbits");
        let posix_like = base.join("nobits");
        std::fs::create_dir_all(&tcc_like).unwrap();
        std::fs::create_dir_all(&posix_like).unwrap();

        // Owner's r+x bits present: a hypothetical denial must come from TCC.
        assert_eq!(classify_denial(&tcc_like), ReadErrorKind::PermissionTcc);

        // Owner bits stripped: POSIX itself denies us.
        let mut perms = std::fs::metadata(&posix_like).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&posix_like, perms).unwrap();
        assert_eq!(classify_denial(&posix_like), ReadErrorKind::PermissionPosix);

        // Through the OS-error mapper both errnos take the same path-based call.
        assert_eq!(
            classify_io_error_at(
                &io::Error::from_raw_os_error(libc::EACCES),
                &posix_like
            ),
            ReadErrorKind::PermissionPosix
        );

        let mut perms = std::fs::metadata(&posix_like).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&posix_like, perms).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn missing_entry_errno_classifies_as_not_found() {
        assert_eq!(
            classify_io_error_at(
                &io::Error::from_raw_os_error(libc::ENOENT),
                Path::new("/nonexistent")
            ),
            ReadErrorKind::NotFound
        );
    }

    /// Exhaust the budget through the non-blocking path: no slot is granted
    /// past the cap, and releasing every taken slot restores the starting
    /// count (every claim is paired with a release).
    #[cfg(target_os = "macos")]
    #[test]
    fn fd_budget_is_capped_and_symmetric() {
        let start = fd_budget::held();
        let mut taken = 0;
        while fd_budget::try_acquire() {
            taken += 1;
            assert!(fd_budget::held() <= fd_budget::limit());
        }
        assert_eq!(fd_budget::held(), fd_budget::limit(), "budget fully held");
        assert!(
            !fd_budget::try_acquire(),
            "no descriptor slot past the cap"
        );
        for _ in 0..taken {
            fd_budget::release();
        }
        assert_eq!(fd_budget::held(), start, "net held count must be zero");
    }
}
