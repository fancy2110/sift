//! Scan-order priority: keep the directory the user is looking at usable
//! within a few hundred milliseconds of starting a full-volume scan.
//!
//! Categories, from soonest to latest:
//!
//! 0. the focus directory itself;
//! 1. its ancestor chain — deeper ancestors first, so the immediate parent
//!    (whose listing reveals the focus) is read before the root;
//! 2. siblings of the focus directory;
//! 3. descendants of the focus directory;
//! 4. everything else, breadth-first so the whole volume fills in evenly.
//!
//! The ordering is a pure function of the path and the focus, which keeps it
//! testable and lets the work queue re-prioritize live when the focus moves.

use std::cmp::Ordering;
use std::path::Path;

/// Sooner is better. Lower value = popped first.
pub fn category(path: &Path, focus: &Path) -> u8 {
    if path == focus {
        0
    } else if focus.starts_with(path) {
        1 // ancestor
    } else if path.parent() == focus.parent() {
        2 // sibling
    } else if path.starts_with(focus) {
        3 // descendant
    } else {
        4
    }
}

/// What a worker should do with this job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Read the directory and let the coordinator build tree nodes for it.
    Tracked,
    /// Read the whole subtree and return only the summed size: the node budget
    /// refused to record this directory, but its bytes still count.
    SizeOnly,
}

/// One entry in the shared work queue.
#[derive(Debug, Clone)]
pub struct QueuedDir {
    /// Index of this directory in the scan tree (for `Tracked`) or of the
    /// parent that must receive the summed size (for `SizeOnly`).
    pub index: u32,
    pub path: std::path::PathBuf,
    pub depth: u16,
    /// Insertion order, for stable tie-breaking.
    pub seq: u64,
    pub kind: JobKind,
}

#[derive(Debug, Clone)]
pub struct OrdDir {
    pub queued: QueuedDir,
    pub category: u8,
    pub focus: std::path::PathBuf,
}

impl PartialEq for OrdDir {
    fn eq(&self, other: &Self) -> bool {
        self.queued.seq == other.queued.seq
    }
}
impl Eq for OrdDir {}

impl PartialOrd for OrdDir {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdDir {
    /// BinaryHeap is a max-heap, so `cmp` is inverted: "better" (smaller
    /// category, then depth per category rules) must compare `Greater`.
    fn cmp(&self, other: &Self) -> Ordering {
        // Category: lower is better → other.cat.cmp(self.cat) is inverted.
        match other.category.cmp(&self.category) {
            Ordering::Equal => {}
            inverted => return inverted,
        }
        // Depth: category 1 wants deeper ancestors first (reverse); everything
        // else wants shallower first (natural).
        match other.queued.depth.cmp(&self.queued.depth) {
            Ordering::Equal => {}
            reversed if self.category == 1 => return reversed.reverse(),
            natural => return natural,
        }
        // Stable insertion order.
        other.queued.seq.cmp(&self.queued.seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn q(path: &str, depth: u16, seq: u64) -> OrdDir {
        OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from(path),
                depth,
                seq,
                kind: JobKind::Tracked,
            },
            category: 0,
            focus: PathBuf::from("/root/a/focus"),
        }
    }

    #[test]
    fn categories_match_the_spec() {
        let focus = Path::new("/root/a/focus");
        assert_eq!(category(Path::new("/root/a/focus"), focus), 0);
        assert_eq!(category(Path::new("/root/a"), focus), 1);
        assert_eq!(category(Path::new("/root"), focus), 1);
        assert_eq!(category(Path::new("/root/a/sib"), focus), 2);
        assert_eq!(category(Path::new("/root/a/focus/sub"), focus), 3);
        assert_eq!(category(Path::new("/root/b"), focus), 4);
        assert_eq!(category(Path::new("/other"), focus), 4);
    }

    #[test]
    fn focus_beats_everything_else() {
        let focus = PathBuf::from("/root/a/focus");
        let mut heap = std::collections::BinaryHeap::new();
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root/b"),
                depth: 1,
                seq: 1,
                kind: JobKind::Tracked,
            },
            category: 4,
            focus: focus.clone(),
        });
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root/a/focus"),
                depth: 3,
                seq: 2,
                kind: JobKind::Tracked,
            },
            category: 0,
            focus: focus.clone(),
        });
        let first = heap.pop().unwrap();
        assert_eq!(first.queued.path, PathBuf::from("/root/a/focus"));
    }

    #[test]
    fn deeper_ancestors_come_first() {
        let focus = PathBuf::from("/root/a/focus");
        let mut heap = std::collections::BinaryHeap::new();
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root"),
                depth: 0,
                seq: 1,
                kind: JobKind::Tracked,
            },
            category: 1,
            focus: focus.clone(),
        });
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root/a"),
                depth: 1,
                seq: 2,
                kind: JobKind::Tracked,
            },
            category: 1,
            focus: focus.clone(),
        });
        assert_eq!(heap.pop().unwrap().queued.path, PathBuf::from("/root/a"));
    }

    #[test]
    fn shallower_rest_dirs_come_first() {
        let focus = PathBuf::from("/root/a/focus");
        let mut heap = std::collections::BinaryHeap::new();
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root/b/deep/deeper"),
                depth: 4,
                seq: 1,
                kind: JobKind::Tracked,
            },
            category: 4,
            focus: focus.clone(),
        });
        heap.push(OrdDir {
            queued: QueuedDir {
                index: 0,
                path: PathBuf::from("/root/b"),
                depth: 1,
                seq: 2,
                kind: JobKind::Tracked,
            },
            category: 4,
            focus: focus.clone(),
        });
        assert_eq!(heap.pop().unwrap().queued.path, PathBuf::from("/root/b"));
    }

    #[test]
    fn same_priority_keeps_insertion_order() {
        let _focus = PathBuf::from("/root/a/focus");
        let mut heap = std::collections::BinaryHeap::new();
        heap.push(q("/root/c", 1, 1));
        heap.push(q("/root/d", 1, 2));
        // Category defaults to 0 in `q`, so both are (0, depth 1).
        assert_eq!(heap.pop().unwrap().queued.path, PathBuf::from("/root/c"));
        assert_eq!(heap.pop().unwrap().queued.path, PathBuf::from("/root/d"));
    }
}
