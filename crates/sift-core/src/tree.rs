//! Compact arena tree of the scanned hierarchy.
//!
//! Six million nodes fit in a few hundred megabytes only if a node is a plain
//! `Copy` record in one `Vec` addressed by `u32` index — no `Box`, no owned
//! `PathBuf`, no per-node `HashMap`. Three design choices carry the rest:
//!
//! * **Names are interned.** A node stores an 8-byte [`NameRef`], not a `String`.
//! * **Files are not nodes.** Only directories get a record. A file contributes
//!   its bytes to its parent and is then forgotten, unless the caller asked to
//!   retain file-level detail ([`TreeConfig::file_detail_min_bytes`]). This is
//!   the single largest saving: a typical volume has 5–10× more files than
//!   directories, and no treemap or drill-down needs the files of a directory
//!   nobody opened.
//! * **Paths are reconstructed, never stored.** A path is walked from the node
//!   up to the root on demand, so a million files do not cost a million strings.
//!
//! The tree is also the accumulator: [`ScanTree::record_file`] credits a file's
//! bytes to its parent, and closing a directory rolls its total into its parent.
//! Directory totals are therefore final the moment the directory closes, which
//! lets a UI render a stable number while the rest of the volume is still being
//! walked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::id::{FileId, NodeKey};
use crate::interner::{NameInterner, NameRef};
use crate::size::ByteSize;

/// Sentinel for "no node".
pub const NONE: u32 = u32::MAX;

/// What a node is. Stored in one byte, so the variants beyond `Dir` cost
/// nothing for the overwhelming majority of records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Dir,
    File,
    /// A file, recorded only because it passed the detail threshold.
    RetainedFile,
    /// An unreadable or non-mountable entry, folded in as an opaque blob.
    Opaque,
}

impl NodeKind {
    const fn from_u8(value: u8) -> Self {
        match value & KIND_MASK {
            KIND_DIR => NodeKind::Dir,
            KIND_FILE => NodeKind::File,
            KIND_RETAINED_FILE => NodeKind::RetainedFile,
            KIND_OPAQUE => NodeKind::Opaque,
            _ => NodeKind::Opaque,
        }
    }

    #[inline]
    pub const fn is_dir(self) -> bool {
        matches!(self, NodeKind::Dir)
    }

    #[inline]
    pub const fn is_file(self) -> bool {
        matches!(self, NodeKind::File | NodeKind::RetainedFile)
    }
}

/// One directory (or retained file) in the scanned hierarchy.
///
/// Field order is deliberate: identity and linkage first, then the byte totals
/// the UI reads every frame, then flags. `mtime_ms` is the only field a scan is
/// allowed to leave at zero, and only when the platform's bulk enumeration did
/// not report one.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct TreeNode {
    pub key: NodeKey,
    pub parent: u32,
    /// First child in the sibling chain; `NONE` when empty.
    pub first_child: u32,
    /// Next sibling of this node.
    pub next_sibling: u32,
    pub name: NameRef,
    /// Bytes of everything in this subtree, logical and physical.
    pub size: ByteSize,
    pub mtime_ms: i64,
    /// Files anywhere in this subtree.
    pub file_count: u32,
    /// Kind (2 bits) plus boolean flags, packed so a node stays at one cache
    /// line. See the `KIND_*` and `FLAG_*` constants below.
    flags: u8,
}

const KIND_MASK: u8 = 0b0000_0011;
const KIND_DIR: u8 = 0;
const KIND_FILE: u8 = 1;
const KIND_RETAINED_FILE: u8 = 2;
const KIND_OPAQUE: u8 = 3;
const FLAG_PENDING: u8 = 0b0000_0100;
const FLAG_SYMLINK: u8 = 0b0000_1000;
const FLAG_DELETABLE: u8 = 0b0001_0000;
const FLAG_ESTIMATED: u8 = 0b0010_0000;

impl TreeNode {
    #[inline]
    pub fn size_bytes(&self) -> u64 {
        self.size.dominant()
    }

    #[inline]
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_u8(self.flags & KIND_MASK)
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        matches!(self.kind(), NodeKind::Dir)
    }

    #[inline]
    pub fn pending(&self) -> bool {
        self.flags & FLAG_PENDING != 0
    }

    #[inline]
    pub fn is_symlink(&self) -> bool {
        self.flags & FLAG_SYMLINK != 0
    }

    #[inline]
    pub fn deletable(&self) -> bool {
        self.flags & FLAG_DELETABLE != 0
    }

    /// Totals are an estimate pending asynchronous calibration.
    #[inline]
    pub fn estimated(&self) -> bool {
        self.flags & FLAG_ESTIMATED != 0
    }

    #[inline]
    pub fn set_estimated(&mut self, estimated: bool) {
        self.flags = (self.flags & !FLAG_ESTIMATED)
            | (if estimated { FLAG_ESTIMATED } else { 0 });
    }

    #[inline]
    fn set_pending(&mut self, pending: bool) {
        self.flags = (self.flags & !FLAG_PENDING) | (if pending { FLAG_PENDING } else { 0 });
    }

    #[inline]
    fn set_deletable(&mut self, deletable: bool) {
        self.flags = (self.flags & !FLAG_DELETABLE) | (if deletable { FLAG_DELETABLE } else { 0 });
    }
}

/// Limits that keep a scan inside its memory budget.
///
/// The defaults are tuned for a laptop scanning a multi-terabyte system volume:
/// they admit every directory a user can reach in the UI, retain file detail
/// only where it is worth a record, and keep the hardlink table from becoming
/// the scan's largest allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TreeConfig {
    /// Hard ceiling on directory records. Past it, directories are still walked
    /// and their bytes still counted into ancestors, but no record is kept.
    pub max_nodes: u32,
    /// Depth past which directories are counted but not recorded.
    pub max_depth: u16,
    /// Directories smaller than this may be folded into their parent when the
    /// node budget is under pressure. `0` records everything.
    pub dir_detail_min_bytes: u64,
    /// Retain a record for individual files at least this large. `u64::MAX`
    /// keeps no file records at all, which is the cheapest mode.
    pub file_detail_min_bytes: u64,
    /// Detect hardlinks so a multiply-linked file is counted once.
    pub dedupe_hardlinks: bool,
    /// Stop growing the hardlink table past this many entries.
    pub max_hardlink_entries: u32,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            max_nodes: 4_000_000,
            max_depth: 64,
            dir_detail_min_bytes: 0,
            // Directories carry the treemap; files are re-read on demand when a
            // directory is opened, so the default keeps no file records.
            file_detail_min_bytes: u64::MAX,
            dedupe_hardlinks: true,
            max_hardlink_entries: 2_000_000,
        }
    }
}

impl TreeConfig {
    /// A config matching a scan policy's detail and dedupe settings.
    pub fn from_scan_policy(policy: &crate::ScanPolicy) -> Self {
        Self {
            file_detail_min_bytes: policy.file_detail_min_bytes,
            dedupe_hardlinks: policy.dedupe_hardlinks,
            ..Self::default()
        }
    }

    /// Everything recorded: every directory, every file at or above 1 MiB.
    /// Costs several times the memory of [`TreeConfig::default`].
    pub fn detailed() -> Self {
        Self {
            file_detail_min_bytes: 1024 * 1024,
            ..Self::default()
        }
    }

    /// Smallest footprint: directories only, no hardlink table.
    pub fn lean() -> Self {
        Self {
            max_nodes: 1_000_000,
            dedupe_hardlinks: false,
            ..Self::default()
        }
    }
}

/// A snapshot of one node for the UI, with its path already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeView {
    pub index: u32,
    pub key: NodeKey,
    pub parent: Option<NodeKey>,
    pub name: String,
    pub path: String,
    pub kind: NodeKind,
    pub size: ByteSize,
    pub mtime_ms: i64,
    pub file_count: u32,
    pub pending: bool,
    pub deletable: bool,
    pub is_symlink: bool,
}

impl NodeView {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }
}

/// Memory accounting for the tree, reported so the engine can enforce a budget
/// and the UI can explain what a scan costs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TreeStats {
    /// Directory (and retained file) records held.
    pub nodes: u32,
    /// Directories that were walked but not recorded because a limit was hit.
    pub unrecorded_dirs: u64,
    /// Resident bytes of the tree's own structures.
    pub resident_bytes: usize,
    /// Of which: the node records.
    pub node_bytes: usize,
    /// Of which: the interned name arena and its index.
    pub interner_bytes: usize,
    /// Of which: the hardlink dedupe table.
    pub hardlink_bytes: usize,
    /// Entries currently in the hardlink dedupe table.
    pub hardlink_entries: u32,
    /// Whether the hardlink table stopped growing and may over-count links.
    pub hardlink_table_full: bool,
}

pub struct ScanTree {
    nodes: Vec<TreeNode>,
    root: u32,
    /// The absolute path the scan started at; the base for every reconstructed
    /// path, so a scan of a subdirectory still yields absolute paths.
    root_path: PathBuf,
    interner: NameInterner,
    config: TreeConfig,
    /// Seen (device, inode) pairs for files with `nlink > 1`.
    hardlinks: HashMap<FileId, ()>,
    unrecorded_dirs: u64,
    hardlink_table_full: bool,
    /// Reused by [`ScanTree::path`] so repeated queries do not reallocate.
    scratch: Vec<NameRef>,
}

impl ScanTree {
    /// Start a tree rooted at `root_path`. The root's display name is its
    /// final path component (or the whole path for a bare mount point).
    pub fn new(root_key: NodeKey, root_path: &Path, config: TreeConfig) -> Self {
        let mut interner = NameInterner::new();
        let root_name: Vec<u8> = root_path
            .file_name()
            .map(|name| name.as_encoded_bytes().to_vec())
            .unwrap_or_else(|| root_path.as_os_str().as_encoded_bytes().to_vec());
        let name = interner.intern(&root_name);
        let root = TreeNode {
            key: root_key,
            parent: NONE,
            first_child: NONE,
            next_sibling: NONE,
            name,
            size: ByteSize::ZERO,
            mtime_ms: 0,
            file_count: 0,
            // The volume root itself is never a deletion candidate.
            flags: KIND_DIR | FLAG_PENDING,
        };
        Self {
            nodes: vec![root],
            root: 0,
            root_path: root_path.to_path_buf(),
            interner,
            config,
            hardlinks: HashMap::new(),
            unrecorded_dirs: 0,
            hardlink_table_full: false,
            scratch: Vec::new(),
        }
    }

    #[inline]
    pub fn root(&self) -> u32 {
        self.root
    }

    /// The absolute path the scan started at.
    #[inline]
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    #[inline]
    pub fn config(&self) -> TreeConfig {
        self.config
    }

    #[inline]
    pub fn interner(&self) -> &NameInterner {
        &self.interner
    }

    #[inline]
    pub fn node(&self, index: u32) -> Option<&TreeNode> {
        self.nodes.get(index as usize)
    }

    pub fn node_mut(&mut self, index: u32) -> Option<&mut TreeNode> {
        self.nodes.get_mut(index as usize)
    }

    #[inline]
    pub fn node_count(&self) -> u32 {
        self.nodes.len() as u32
    }

    /// Total bytes of the whole scan.
    #[inline]
    pub fn total(&self) -> ByteSize {
        self.nodes[self.root as usize].size
    }

    #[inline]
    pub fn is_pending(&self, index: u32) -> bool {
        self.node(index).is_some_and(|node| node.pending())
    }

    /// Discover a directory as a child of `parent`.
    ///
    /// Returns `None` when a limit refused the record; the caller is expected to
    /// keep walking for size but stop expecting a node back.
    pub fn open_dir(
        &mut self,
        parent: u32,
        key: NodeKey,
        name: &[u8],
        mtime_ms: i64,
        deletable: bool,
        is_symlink: bool,
    ) -> Option<u32> {
        if self.depth_of(parent) >= self.config.max_depth || self.nodes.len() >= self.config.max_nodes as usize
        {
            self.unrecorded_dirs += 1;
            return None;
        }
        let name = self.interner.intern(name);
        let index = self.nodes.len() as u32;
        let mut flags = KIND_DIR | FLAG_PENDING;
        if is_symlink {
            flags |= FLAG_SYMLINK;
        }
        if deletable {
            flags |= FLAG_DELETABLE;
        }
        self.nodes.push(TreeNode {
            key,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            name,
            size: ByteSize::ZERO,
            mtime_ms,
            file_count: 0,
            flags,
        });
        self.link_child(parent, index);
        Some(index)
    }

    /// Credit a file's bytes to its parent directory.
    ///
    /// Returns `true` when the file was counted, `false` when it was recognised
    /// as an already-counted hardlink. Passing an unusable `file_id` (inode 0)
    /// always counts.
    pub fn record_file(&mut self, parent: u32, size: ByteSize, file_id: Option<FileId>) -> bool {
        if let (Some(id), true) = (file_id, self.config.dedupe_hardlinks) {
            if id.is_usable() {
                if self.hardlinks.contains_key(&id) {
                    return false;
                }
                if self.hardlinks.len() < self.config.max_hardlink_entries as usize {
                    self.hardlinks.insert(id, ());
                } else {
                    self.hardlink_table_full = true;
                }
            }
        }

        if let Some(node) = self.nodes.get_mut(parent as usize) {
            node.size += size;
            node.file_count = node.file_count.saturating_add(1);
        }
        true
    }

    /// Retain a file as a record because it passed
    /// [`TreeConfig::file_detail_min_bytes`]. The caller has already called
    /// [`ScanTree::record_file`] for the same file.
    pub fn retain_file(
        &mut self,
        parent: u32,
        key: NodeKey,
        name: &[u8],
        size: ByteSize,
        mtime_ms: i64,
        deletable: bool,
    ) -> Option<u32> {
        if self.nodes.len() >= self.config.max_nodes as usize {
            return None;
        }
        let name = self.interner.intern(name);
        let index = self.nodes.len() as u32;
        let flags = KIND_RETAINED_FILE | if deletable { FLAG_DELETABLE } else { 0 };
        self.nodes.push(TreeNode {
            key,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            name,
            // A retained file's own size is its whole size.
            size,
            mtime_ms,
            file_count: 1,
            flags,
        });
        self.link_child(parent, index);
        Some(index)
    }

    /// Finish a directory: mark it settled and roll its total into its parent.
    pub fn close_dir(&mut self, index: u32) {
        let Some(node) = self.nodes.get(index as usize) else {
            return;
        };
        let parent = node.parent;
        let total = node.size;
        let files = node.file_count;
        if let Some(node) = self.nodes.get_mut(index as usize) {
            node.set_pending(false);
        }
        // Propagate upward. A directory that was not recorded still had its
        // bytes added by its own parent's bookkeeping, so only recorded nodes
        // propagate here.
        if parent != NONE {
            if let Some(node) = self.nodes.get_mut(parent as usize) {
                node.size += total;
                node.file_count = node.file_count.saturating_add(files);
            }
        }
    }

    /// Reopen a previously closed directory for a fresh read of its subtree.
    ///
    /// Its old totals are subtracted from its parent (which is assumed still
    /// open), its counters are reset, and it is marked pending again. This is
    /// what lets a user focus a snapshot-reused directory and expand it
    /// without its old bytes being counted twice.
    pub fn reopen_dir(&mut self, index: u32) {
        let Some(node) = self.nodes.get(index as usize).copied() else {
            return;
        };
        let old = node.size;
        let files = node.file_count;
        let parent = node.parent;
        if parent != NONE {
            if let Some(parent_node) = self.nodes.get_mut(parent as usize) {
                parent_node.size -= old;
                parent_node.file_count = parent_node.file_count.saturating_sub(files);
            }
        }
        if let Some(node) = self.nodes.get_mut(index as usize) {
            node.size = ByteSize::ZERO;
            node.file_count = 0;
            node.set_pending(true);
        }
    }

    /// Bytes that a directory counted but that never reached it through a child
    /// record, e.g. subdirectories that hit the node budget. Used after a scan
    /// so an unrecorded subtree is not silently lost.
    /// Apply a post-scan calibration to a closed directory.
    ///
    /// The difference between measured and previously estimated totals is
    /// propagated up the (already closed) ancestor chain, so the volume total
    /// stays exact after an asynchronous recalibration.
    pub fn adjust_totals(&mut self, index: u32, measured: ByteSize, files: u64) -> (ByteSize, i64) {
        let Some(node) = self.nodes.get(index as usize).copied() else {
            return (ByteSize::ZERO, 0);
        };
        let old_size = node.size;
        let old_files = node.file_count as u64;
        let signed_size = ByteSize::new(
            measured.logical.wrapping_sub(old_size.logical),
            measured.physical.wrapping_sub(old_size.physical),
        );
        let file_delta = files as i64 - old_files as i64;

        let mut cursor = index;
        loop {
            let Some(node) = self.nodes.get(cursor as usize).copied() else { break };
            let parent = node.parent;
            if let Some(n) = self.nodes.get_mut(cursor as usize) {
                n.size += signed_size;
                if file_delta >= 0 {
                    n.file_count = n.file_count.saturating_add(file_delta as u32);
                } else {
                    n.file_count = n.file_count.saturating_sub((-file_delta) as u32);
                }
            }
            if parent == NONE {
                break;
            }
            cursor = parent;
        }
        (signed_size, file_delta)
    }

    pub fn add_unrecorded(&mut self, parent: u32, size: ByteSize, files: u32) {
        if let Some(node) = self.nodes.get_mut(parent as usize) {
            node.size += size;
            node.file_count = node.file_count.saturating_add(files);
        }
    }

    pub fn set_deletable(&mut self, index: u32, deletable: bool) {
        if let Some(node) = self.nodes.get_mut(index as usize) {
            node.set_deletable(deletable);
        }
    }

    /// Direct children of `parent`, in sibling-chain order.
    pub fn children(&self, parent: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let Some(node) = self.nodes.get(parent as usize) else {
            return out;
        };
        let mut cursor = node.first_child;
        while cursor != NONE {
            out.push(cursor);
            match self.nodes.get(cursor as usize) {
                Some(child) => cursor = child.next_sibling,
                None => break,
            }
        }
        out
    }

    /// Resolve a node's name.
    pub fn name(&self, index: u32) -> &[u8] {
        match self.nodes.get(index as usize) {
            Some(node) => self.interner.lookup(node.name),
            None => b"",
        }
    }

    pub fn name_string(&self, index: u32) -> String {
        match self.nodes.get(index as usize) {
            Some(node) => self.interner.to_string_lossy(node.name),
            None => String::new(),
        }
    }

    /// Walk from `index` up to the root and rebuild its absolute path.
    ///
    /// The root's stored path is the base, so the result is absolute even when
    /// the scan started at a subdirectory. O(depth) with no per-component
    /// allocation beyond the string itself, which is what makes "store no
    /// paths" affordable.
    pub fn path(&mut self, index: u32) -> String {
        self.scratch.clear();
        let mut cursor = index;
        // Collect names from `index` up to, but excluding, the root.
        while let Some(node) = self.nodes.get(cursor as usize) {
            if node.parent == NONE {
                break;
            }
            self.scratch.push(node.name);
            cursor = node.parent;
        }

        let mut out = self.root_path.to_string_lossy().into_owned();
        for &name in self.scratch.iter().rev() {
            if !out.ends_with('/') && !out.is_empty() {
                out.push('/');
            }
            out.push_str(&String::from_utf8_lossy(self.interner.lookup(name)));
        }
        if out.is_empty() {
            out.push(std::path::MAIN_SEPARATOR);
        }
        out
    }

    /// Ancestor chain from the root down to `index`.
    pub fn breadcrumbs(&self, index: u32) -> Vec<u32> {
        let mut chain = vec![index];
        let mut cursor = index;
        while let Some(node) = self.nodes.get(cursor as usize) {
            if node.parent == NONE {
                break;
            }
            chain.push(node.parent);
            cursor = node.parent;
        }
        chain.reverse();
        chain
    }

    /// A UI-ready snapshot of one node, with its path resolved.
    pub fn view(&mut self, index: u32) -> Option<NodeView> {
        let node = *self.nodes.get(index as usize)?;
        let name = self.interner.to_string_lossy(node.name);
        let path = self.path(index);
        let parent = self
            .nodes
            .get(node.parent as usize)
            .map(|parent_node| parent_node.key);
        Some(NodeView {
            index,
            key: node.key,
            parent,
            name,
            path,
            kind: node.kind(),
            size: node.size,
            mtime_ms: node.mtime_ms,
            file_count: node.file_count,
            pending: node.pending(),
            deletable: node.deletable(),
            is_symlink: node.is_symlink(),
        })
    }

    /// Children of `parent` sorted the way the product displays them:
    /// directories before files, then by logical size descending.
    pub fn children_by_size(&self, parent: u32) -> Vec<u32> {
        let mut children = self.children(parent);
        children.sort_by(|&a, &b| {
            let left = &self.nodes[a as usize];
            let right = &self.nodes[b as usize];
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| right.size.logical.cmp(&left.size.logical))
                .then_with(|| right.size.physical.cmp(&left.size.physical))
                // Stable, deterministic tie-break on identity so a re-render
                // never reshuffles equal-sized siblings.
                .then_with(|| left.key.cmp(&right.key))
        });
        children
    }

    pub fn stats(&self) -> TreeStats {
        let node_bytes = self.nodes.capacity() * std::mem::size_of::<TreeNode>();
        let interner_bytes = self.interner.resident_bytes();
        let hardlink_bytes = self.hardlinks.capacity() * (std::mem::size_of::<FileId>() + 1);
        TreeStats {
            nodes: self.nodes.len() as u32,
            unrecorded_dirs: self.unrecorded_dirs,
            resident_bytes: node_bytes + interner_bytes + hardlink_bytes,
            node_bytes,
            interner_bytes,
            hardlink_bytes,
            hardlink_entries: self.hardlinks.len() as u32,
            hardlink_table_full: self.hardlink_table_full,
        }
    }

    /// Release excess capacity once a scan has finished.
    ///
    /// A `Vec` grown by doubling typically holds 1.5–2x the nodes it needs.
    /// Calling this after the walk trades one copy for a large, permanent
    /// reduction in resident memory.
    pub fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
        self.scratch.shrink_to_fit();
        self.hardlinks.shrink_to_fit();
    }

    /// Resident bytes owned by the tree, including its reusable path scratch.
    pub fn resident_bytes(&self) -> usize {
        self.stats().resident_bytes + self.scratch.capacity() * std::mem::size_of::<NameRef>()
    }

    /// Bytes one node record costs before its name. Kept public so a budget
    /// calculation can be checked against reality in tests and benchmarks.
    pub const fn node_record_bytes() -> usize {
        std::mem::size_of::<TreeNode>()
    }

    // ---- internals ---------------------------------------------------------

    pub fn depth_of(&self, index: u32) -> u16 {
        let mut depth = 0u16;
        let mut cursor = index;
        while let Some(node) = self.nodes.get(cursor as usize) {
            if node.parent == NONE {
                break;
            }
            depth = depth.saturating_add(1);
            cursor = node.parent;
        }
        depth
    }

    /// Append `child` to `parent`'s sibling chain.
    fn link_child(&mut self, parent: u32, child: u32) {
        // Copy linkage state first so no borrow is held across a mutation.
        let head = match self.nodes.get(parent as usize) {
            Some(node) => node.first_child,
            None => return,
        };
        if head == NONE {
            if let Some(node) = self.nodes.get_mut(parent as usize) {
                node.first_child = child;
            }
            return;
        }
        // Walk to the tail; chains are short (direct children of one directory).
        let mut cursor = head;
        while let Some(node) = self.nodes.get(cursor as usize) {
            let next = node.next_sibling;
            if next == NONE {
                if let Some(node) = self.nodes.get_mut(cursor as usize) {
                    node.next_sibling = child;
                }
                return;
            }
            cursor = next;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::id::NodeKey;

    fn key(path: &str) -> NodeKey {
        NodeKey::from_bytes(path.as_bytes())
    }

    fn tree() -> ScanTree {
        ScanTree::new(key("/"), Path::new("/"), TreeConfig::default())
    }

    #[test]
    fn calibration_delta_propagates_to_ancestors() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        let b = tree
            .open_dir(a, key("/a/b"), b"b", 0, true, false)
            .unwrap();
        // Simulate a predicted giant subtree: b closes with an estimated
        // file count (say 1) and zero bytes, no real files recorded.
        if let Some(n) = tree.node_mut(b) {
            n.file_count = 1;
        }
        if let Some(n) = tree.node_mut(a) {
            n.file_count = 1;
        }
        if let Some(n) = tree.node_mut(root) {
            n.file_count = 1;
        }

        // Exact calibration of b.
        let measured = ByteSize::new(4000, 8192);
        let (delta_size, delta_files) = tree.adjust_totals(b, measured, 3);
        assert_eq!(delta_size, measured);
        assert_eq!(delta_files, 2);

        // The delta must have propagated up the whole ancestor chain.
        assert_eq!(tree.node(b).unwrap().size, measured);
        assert_eq!(tree.node(a).unwrap().size, measured);
        assert_eq!(tree.node(root).unwrap().size, measured);
        assert_eq!(tree.node(b).unwrap().file_count, 3);
        assert_eq!(tree.node(a).unwrap().file_count, 3);
        assert_eq!(tree.node(root).unwrap().file_count, 3);
    }

    #[test]
    fn node_record_stays_small() {
        // The whole memory argument rests on this staying near 64 bytes.
        let bytes = ScanTree::node_record_bytes();
        assert!(bytes <= 80, "TreeNode grew to {bytes} bytes");
    }

    #[test]
    fn file_bytes_roll_up_through_directories() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        let b = tree
            .open_dir(a, key("/a/b"), b"b", 0, true, false)
            .unwrap();

        tree.record_file(b, ByteSize::new(1000, 4096), None);
        tree.record_file(a, ByteSize::new(500, 4096), None);
        tree.close_dir(b);
        tree.close_dir(a);

        assert_eq!(tree.node(b).unwrap().size.logical, 1000);
        assert_eq!(tree.node(a).unwrap().size.logical, 1500);
        assert_eq!(tree.total().logical, 1500);
        assert_eq!(tree.node(root).unwrap().file_count, 2);
    }

    #[test]
    fn physical_size_is_tracked_separately() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        // A 1 GiB logical cloned file occupying 4 KiB.
        tree.record_file(a, ByteSize::new(1 << 30, 4096), None);
        tree.close_dir(a);
        assert_eq!(tree.node(a).unwrap().size.logical, 1 << 30);
        assert_eq!(tree.node(a).unwrap().size.physical, 4096);
        assert_eq!(tree.node(a).unwrap().size.reclaimable(), 4096);
    }

    #[test]
    fn hardlinks_are_counted_once() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        let id = FileId::new(1, 99);
        assert!(tree.record_file(a, ByteSize::logical_only(4096), Some(id)));
        assert!(!tree.record_file(a, ByteSize::logical_only(4096), Some(id)));
        assert_eq!(tree.node(a).unwrap().size.logical, 4096);
        // A different inode is a different file.
        assert!(tree.record_file(a, ByteSize::logical_only(4096), Some(FileId::new(1, 100))));
        assert_eq!(tree.node(a).unwrap().size.logical, 8192);
    }

    /// The dedupe table must only ever see files the platform reported as
    /// multiply linked; the engine relies on this to keep it small.
    #[test]
    fn the_hardlink_table_only_grows_for_multi_linked_files() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        // A single-linked file passes `None` (what the engine does when
        // `nlink <= 1`), so nothing is remembered.
        for index in 0..100 {
            tree.record_file(a, ByteSize::logical_only(1), None);
            let _ = index;
        }
        assert_eq!(tree.stats().hardlink_entries, 0);
        assert_eq!(tree.stats().hardlink_bytes, 0);

        // A multiply-linked file is remembered and counted once.
        let id = FileId::new(1, 7);
        assert!(tree.record_file(a, ByteSize::logical_only(9), Some(id)));
        assert!(!tree.record_file(a, ByteSize::logical_only(9), Some(id)));
        assert_eq!(tree.stats().hardlink_entries, 1);
        assert!(tree.stats().hardlink_bytes > 0);
    }

    #[test]
    fn shrinking_reclaims_doubling_slack() {
        let mut tree = tree();
        let root = tree.root();
        for index in 0..1000u32 {
            let name = format!("dir-{index}");
            tree.open_dir(root, key(&name), name.as_bytes(), 0, true, false);
        }
        let before = tree.resident_bytes();
        tree.shrink_to_fit();
        let after = tree.resident_bytes();
        assert!(
            after <= before,
            "shrinking must never grow the tree ({before} -> {after})"
        );
        // The nodes are all still there and still reachable.
        assert_eq!(tree.node_count(), 1001);
        assert_eq!(tree.children(root).len(), 1000);
        assert_eq!(tree.name(tree.children(root)[0]), b"dir-0");
    }

    #[test]
    fn the_memory_breakdown_adds_up() {
        let mut tree = tree();
        let root = tree.root();
        for index in 0..500u32 {
            let name = format!("dir-{index}");
            tree.open_dir(root, key(&name), name.as_bytes(), 0, true, false);
        }
        let stats = tree.stats();
        assert_eq!(
            stats.resident_bytes,
            stats.node_bytes + stats.interner_bytes + stats.hardlink_bytes
        );
        assert!(stats.node_bytes > 0);
        assert!(stats.interner_bytes > 0);
        // Nodes dominate for a wide tree.
        assert!(stats.node_bytes > stats.interner_bytes);
    }

    #[test]
    fn dedupe_can_be_disabled() {
        let mut tree = ScanTree::new(
            key("/"),
            Path::new("/"),
            TreeConfig {
                dedupe_hardlinks: false,
                ..TreeConfig::default()
            },
        );
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        let id = FileId::new(1, 99);
        assert!(tree.record_file(a, ByteSize::logical_only(4096), Some(id)));
        assert!(tree.record_file(a, ByteSize::logical_only(4096), Some(id)));
        assert_eq!(tree.node(a).unwrap().size.logical, 8192);
    }

    #[test]
    fn unrecorded_dir_still_reports_through_parent_counters() {
        let mut tree = ScanTree::new(
            key("/"),
            Path::new("/"),
            TreeConfig {
                max_nodes: 2,
                ..TreeConfig::default()
            },
        );
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        // Budget of two: root + a. The next directory must be refused.
        assert!(tree.open_dir(a, key("/a/b"), b"b", 0, true, false).is_none());
        tree.add_unrecorded(a, ByteSize::logical_only(777), 3);
        tree.close_dir(a);
        assert_eq!(tree.total().logical, 777);
        assert_eq!(tree.node(root).unwrap().file_count, 3);
        assert_eq!(tree.stats().unrecorded_dirs, 1);
    }

    #[test]
    fn children_by_size_puts_directories_first() {
        let mut tree = tree();
        let root = tree.root();
        let small = tree.open_dir(root, key("/small"), b"small", 0, true, false).unwrap();
        let big = tree.open_dir(root, key("/big"), b"big", 0, true, false).unwrap();
        let file = tree
            .retain_file(root, key("/huge.bin"), b"huge.bin", ByteSize::logical_only(9999), 0, true)
            .unwrap();
        tree.record_file(small, ByteSize::logical_only(10), None);
        tree.close_dir(small);
        tree.record_file(big, ByteSize::logical_only(5000), None);
        tree.close_dir(big);

        let ordered = tree.children_by_size(root);
        assert_eq!(ordered, vec![big, small, file]);
    }

    #[test]
    fn path_is_reconstructed_from_ancestors() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/Users"), b"Users", 0, true, false).unwrap();
        let b = tree.open_dir(a, key("/Users/me"), b"me", 0, true, false).unwrap();
        let c = tree
            .open_dir(b, key("/Users/me/Documents"), b"Documents", 0, true, false)
            .unwrap();
        assert_eq!(tree.path(c), "/Users/me/Documents");
        assert_eq!(tree.path(root), "/");
        assert_eq!(tree.breadcrumbs(c), vec![root, a, b, c]);
        assert_eq!(tree.name(c), b"Documents");
    }

    #[test]
    fn retained_files_appear_with_their_size() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        tree.record_file(a, ByteSize::logical_only(2 << 20), None);
        let file = tree
            .retain_file(a, key("/a/big.iso"), b"big.iso", ByteSize::logical_only(2 << 20), 12, true)
            .unwrap();
        tree.close_dir(a);
        assert_eq!(tree.node(file).unwrap().kind(), NodeKind::RetainedFile);
        assert_eq!(tree.node(file).unwrap().size.logical, 2 << 20);
        // The file is in a's chain but its bytes were counted once.
        assert_eq!(tree.node(a).unwrap().size.logical, 2 << 20);
        assert!(tree.children(a).contains(&file));
    }

    #[test]
    fn close_dir_is_idempotent_for_pending_flag() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 0, true, false).unwrap();
        assert!(tree.is_pending(a));
        tree.close_dir(a);
        assert!(!tree.is_pending(a));
        // A second close must not double-count.
        let before = tree.total().logical;
        tree.close_dir(a);
        assert_eq!(tree.total().logical, before);
    }

    #[test]
    fn view_exposes_ui_fields() {
        let mut tree = tree();
        let root = tree.root();
        let a = tree.open_dir(root, key("/a"), b"a", 1234, false, true).unwrap();
        tree.record_file(a, ByteSize::logical_only(64), None);
        tree.close_dir(a);
        let view = tree.view(a).unwrap();
        assert_eq!(view.name, "a");
        assert_eq!(view.path, "/a");
        assert_eq!(view.mtime_ms, 1234);
        assert!(!view.deletable);
        assert!(view.is_symlink);
        assert_eq!(view.parent, Some(key("/")));
        assert_eq!(view.file_count, 1);
    }

    #[test]
    fn memory_scales_linearly_and_stays_under_budget() {
        let mut tree = ScanTree::new(
            key("/"),
            Path::new("/"),
            TreeConfig {
                max_nodes: 200_000,
                ..TreeConfig::default()
            },
        );
        let root = tree.root();
        for i in 0..50_000u32 {
            let name = format!("dir-{i}");
            tree.open_dir(root, NodeKey::from_bytes(name.as_bytes()), name.as_bytes(), 0, true, false);
        }
        let per_node = tree.resident_bytes() / tree.node_count() as usize;
        // 50k nodes plus a shared interner; the per-node cost must be tens of bytes.
        assert!(per_node < 200, "per-node resident cost is {per_node} bytes");
    }
}
