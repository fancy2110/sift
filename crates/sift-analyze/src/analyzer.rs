//! Turning a completed scan into candidates, then verdicts.
//!
//! Two passes, because the two kinds of conclusion need different data:
//!
//! * **Directory pass** — pure tree walk, no filesystem I/O. The scan tree
//!   already holds every directory with its exact rolled-up size, so
//!   `node_modules`, build output, application caches and the trash are found
//!   for free, even on a volume with millions of files.
//! * **File pass** — bounded, on-demand enumeration of a small set of
//!   directories (Downloads, Desktop, …). File-level candidates (installers,
//!   archives, stale files, duplicates) need per-file metadata, and re-reading
//!   a handful of directories is far cheaper than having retained millions of
//!   file records during the scan.
//!
//! This is why the scan engine can drop file details by default: the analysis
//! re-derives what it needs, when it needs it, from the directories that matter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sift_core::deletable::classify;
use sift_core::{ByteSize, ScanTree};
use sift_platform::dir::DirReader;

use crate::adjudicate::{adjudicate_all, Adjudicator, AnalysisReport, AnalyzedItem};
use crate::candidate::{redact_path, Candidate, CandidateKind, Evidence};
use crate::rules::{self, RuleThresholds};

/// Settings that shape what analysis considers and how much of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisPolicy {
    /// Candidates smaller than this are not worth a user's attention.
    pub min_candidate_bytes: u64,
    pub thresholds: RuleThresholds,
    /// Upper bound on candidates handed to adjudication.
    pub max_candidates: usize,
    /// Whether to run the file pass at all.
    pub include_files: bool,
    /// Extra directories to enumerate in the file pass, beyond
    /// [`Analyzer::default_file_search_dirs`].
    pub extra_search_dirs: Vec<PathBuf>,
    /// Cap on directories enumerated in the file pass.
    pub max_search_dirs: usize,
    /// How many candidates go to an adjudicator per call.
    pub batch_size: usize,
}

impl Default for AnalysisPolicy {
    fn default() -> Self {
        Self {
            min_candidate_bytes: 8 * 1024 * 1024,
            thresholds: RuleThresholds::default(),
            max_candidates: 400,
            include_files: true,
            extra_search_dirs: Vec::new(),
            max_search_dirs: 8,
            batch_size: 40,
        }
    }
}

impl AnalysisPolicy {
    /// Directory pass only: fastest, no extra I/O.
    pub fn directory_only() -> Self {
        Self {
            include_files: false,
            ..Self::default()
        }
    }

    pub fn with_min_bytes(mut self, bytes: u64) -> Self {
        self.min_candidate_bytes = bytes;
        self
    }

    pub fn with_search_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.extra_search_dirs = dirs;
        self
    }
}

/// Progress stages, so a UI can say what it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnalysisStage {
    /// Walking the scan tree for directory candidates.
    DirectoryPass,
    /// Enumerating a small set of directories for file candidates.
    FilePass,
    /// Asking the adjudicator.
    Adjudicating,
    /// Finished.
    Done,
}

/// Builds candidates and runs them through an adjudicator.
pub struct Analyzer {
    policy: AnalysisPolicy,
    home: Option<PathBuf>,
}

impl Analyzer {
    pub fn new(policy: AnalysisPolicy, home: Option<PathBuf>) -> Self {
        Self { policy, home }
    }

    pub fn policy(&self) -> &AnalysisPolicy {
        &self.policy
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Directories the file pass looks at by default: the places installers and
    /// disposable archives actually accumulate.
    pub fn default_file_search_dirs(&self) -> Vec<PathBuf> {
        let Some(home) = &self.home else {
            return self.policy.extra_search_dirs.clone();
        };
        let mut dirs: Vec<PathBuf> = ["Downloads", "Desktop", "Documents", "Movies"]
            .iter()
            .map(|name| home.join(name))
            .filter(|path| path.is_dir())
            .collect();
        dirs.extend(self.policy.extra_search_dirs.iter().cloned());
        dirs.truncate(self.policy.max_search_dirs);
        dirs
    }

    /// Directory pass: every directory in the tree that a rule recognizes.
    ///
    /// Nodes are stored parent-before-child, so a matched subtree is skipped by
    /// remembering which parents were already covered — this keeps the pass
    /// O(nodes) with no ancestry walks and no filesystem calls.
    pub fn dir_candidates(&self, tree: &mut ScanTree, now_ms: i64) -> Vec<Candidate> {
        let root_path = tree.root_path().to_path_buf();
        let mut candidates = Vec::new();
        let mut covered: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let node_count = tree.node_count();

        for index in 0..node_count {
            // Copy everything needed before taking a mutable borrow for `path`.
            let Some(node) = tree.node(index) else {
                continue;
            };
            if !node.is_dir() {
                continue;
            }
            let parent = node.parent;
            let size = node.size;
            let mtime_ms = node.mtime_ms;
            let name_ref = node.name;
            let key = node.key;

            // Already inside a matched subtree (e.g. node_modules/foo/node_modules).
            if parent != sift_core::NONE && covered.contains(&parent) {
                covered.insert(index);
                continue;
            }
            if size.dominant() < self.policy.min_candidate_bytes {
                continue;
            }

            let name = tree.interner().to_string_lossy(name_ref);
            let path = PathBuf::from(tree.path(index));
            // Never nominate something the deletion policy already refuses.
            if !classify(&path).is_yes() {
                continue;
            }
            let Some(hit) = rules::for_dir(&name, &path) else {
                continue;
            };

            let mut candidate = Candidate::new(
                key,
                path.clone(),
                redact_path(&path, self.home.as_deref(), Some(&root_path)),
                name,
                true,
                size,
                mtime_ms,
                hit.kind.clone(),
            );
            candidate.evidence.push(hit.evidence());
            candidate = candidate.with_nomination(hit.nomination());
            let _ = now_ms;
            candidates.push(candidate);
            covered.insert(index);
        }

        self.finish(candidates)
    }

    /// File pass: enumerate `dirs` once each and nominate file candidates.
    ///
    /// Duplicate detection is scoped to the directories enumerated here rather
    /// than to the whole volume: a bounded, explainable comparison beats a
    /// volume-wide hash table that would need the file list the scan
    /// deliberately did not keep.
    pub fn file_candidates(&self, dirs: &[PathBuf], now_ms: i64) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        // (name, size) -> the paths seen with that identity.
        let mut seen: HashMap<(String, u64), Vec<SeenCopy>> = HashMap::new();

        for dir in dirs.iter().take(self.policy.max_search_dirs) {
            let Ok(reader) = DirReader::read(dir, true) else {
                continue;
            };
            let entries = reader.into_parts().1;
            for entry in entries {
                let name = String::from_utf8_lossy(&entry.name).into_owned();
                if name == "." || name == ".." {
                    continue;
                }
                let path = dir.join(&name);
                if !classify(&path).is_yes() {
                    continue;
                }
                let size = entry.size();
                if entry.is_dir {
                    continue;
                }
                let key = sift_core::NodeKey::from_path(&path);

                if size.logical >= self.policy.thresholds.duplicate_min_bytes {
                    seen.entry((name.clone(), size.logical))
                        .or_default()
                        .push(SeenCopy {
                            path: path.clone(),
                            size,
                            mtime_ms: entry.mtime_ms,
                            key,
                        });
                }

                if size.dominant() < self.policy.min_candidate_bytes {
                    continue;
                }
                let Some(hit) = rules::for_file(
                    &name,
                    &path,
                    size.dominant(),
                    entry.mtime_ms,
                    now_ms,
                    &self.policy.thresholds,
                ) else {
                    continue;
                };
                let mut candidate = Candidate::new(
                    key,
                    path.clone(),
                    redact_path(&path, self.home.as_deref(), None),
                    name,
                    false,
                    size,
                    entry.mtime_ms,
                    hit.kind.clone(),
                );
                candidate.evidence.push(hit.evidence());
                candidate = candidate.with_nomination(hit.nomination());
                candidates.push(candidate);
            }
        }

        // Duplicate groups: at least two copies, each worth deciding about.
        for ((name, _size), copies) in seen {
            if copies.len() < 2 {
                continue;
            }
            let copy_count = copies.len() as u32;
            for copy in copies {
                let mut candidate = Candidate::new(
                    copy.key,
                    copy.path.clone(),
                    redact_path(&copy.path, self.home.as_deref(), None),
                    name.clone(),
                    false,
                    copy.size,
                    copy.mtime_ms,
                    CandidateKind::DuplicateGroup {
                        name: name.clone(),
                        copies: copy_count,
                    },
                );
                candidate.evidence.push(
                    Evidence::new("file.duplicate", "detail.duplicateCopies")
                        .with_param(copy_count.to_string()),
                );
                candidate = candidate.with_nomination(crate::candidate::Nomination {
                    rule: "file.duplicate".to_string(),
                    // A duplicate set is a question ("which copy do you want?"),
                    // never a standing permission.
                    safety: crate::reason::Safety::Review,
                    reason_key: "reason.duplicate".to_string(),
                    reason_param: Some(name.clone()),
                    detail_key: "detail.duplicateCopies".to_string(),
                });
                candidates.push(candidate);
            }
        }

        self.finish(candidates)
    }

    /// Sort by reclaimable bytes descending, drop sub-threshold entries, and
    /// cap the result so adjudication stays bounded.
    fn finish(&self, mut candidates: Vec<Candidate>) -> Vec<Candidate> {
        candidates.retain(|candidate| candidate.size.dominant() >= self.policy.min_candidate_bytes);
        candidates.sort_by(|left, right| {
            right
                .reclaimable()
                .cmp(&left.reclaimable())
                .then_with(|| left.key.cmp(&right.key))
        });
        candidates.truncate(self.policy.max_candidates);
        candidates
    }

    /// The whole pipeline: candidates, then verdicts, then a report.
    ///
    /// `on_progress` is called with the stage and a `(done, total)` pair so a UI
    /// can show a determinate bar during adjudication.
    pub fn analyze<A: Adjudicator + ?Sized>(
        &self,
        adjudicator: &A,
        tree: &mut ScanTree,
        now_ms: i64,
        mut on_progress: impl FnMut(AnalysisStage, usize, usize),
    ) -> AnalysisReport {
        on_progress(AnalysisStage::DirectoryPass, 0, 1);
        let mut candidates = self.dir_candidates(tree, now_ms);
        on_progress(AnalysisStage::DirectoryPass, 1, 1);

        if self.policy.include_files {
            let dirs = self.default_file_search_dirs();
            on_progress(AnalysisStage::FilePass, 0, dirs.len().max(1));
            let file_candidates = self.file_candidates(&dirs, now_ms);
            on_progress(
                AnalysisStage::FilePass,
                dirs.len().max(1),
                dirs.len().max(1),
            );
            candidates.extend(file_candidates);
            candidates = self.finish(candidates);
        }

        let total = candidates.len();
        on_progress(AnalysisStage::Adjudicating, 0, total.max(1));
        let verdicts = adjudicate_all(
            adjudicator,
            &candidates,
            self.policy.batch_size,
            now_ms,
            |done, _| on_progress(AnalysisStage::Adjudicating, done, total.max(1)),
        );
        on_progress(AnalysisStage::Adjudicating, total, total.max(1));

        let items: Vec<AnalyzedItem> = candidates
            .into_iter()
            .zip(verdicts)
            .map(|(candidate, verdict)| AnalyzedItem::new(candidate, verdict))
            .collect();

        let finished_at_ms = now_ms;
        on_progress(AnalysisStage::Done, 1, 1);
        AnalysisReport::new(items, now_ms, finished_at_ms)
    }
}

/// One file observed during the duplicate scan: path, size, mtime, identity.
#[derive(Debug, Clone)]
struct SeenCopy {
    path: PathBuf,
    size: ByteSize,
    mtime_ms: i64,
    key: sift_core::NodeKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjudicate::{RuleAdjudicator, VerdictCache};
    use crate::reason::{PathFingerprint, Safety};
    use sift_core::{NodeKey, TreeConfig};

    fn tree_over(base: &Path) -> ScanTree {
        ScanTree::new(NodeKey::from_path(base), base, TreeConfig::default())
    }

    /// Build a tree by hand: cheaper and more precise than scanning for a test
    /// that is about rules, not about walking.
    fn add_dir(tree: &mut ScanTree, parent: u32, name: &str, size: u64) -> u32 {
        let path = format!("{}/{}", tree.path(parent), name);
        let index = tree
            .open_dir(
                parent,
                NodeKey::from_bytes(path.as_bytes()),
                name.as_bytes(),
                0,
                true,
                false,
            )
            .expect("open_dir");
        tree.record_file(index, ByteSize::new(size, size), None);
        tree.close_dir(index);
        index
    }

    #[test]
    fn directory_pass_finds_caches_and_skips_nested_matches() {
        let base = std::env::temp_dir().join("sift-analyze-dirpass");
        let mut tree = tree_over(&base);
        let root = tree.root();
        let modules = add_dir(&mut tree, root, "node_modules", 500 * 1024 * 1024);
        // A nested node_modules must not become a second candidate.
        let nested = add_dir(&mut tree, modules, "node_modules", 400 * 1024 * 1024);
        let _ = nested;
        add_dir(&mut tree, root, "src", 1024);
        add_dir(&mut tree, root, "trash", 2048);

        let analyzer = Analyzer::new(AnalysisPolicy::directory_only(), None);
        let candidates = analyzer.dir_candidates(&mut tree, 0);

        assert_eq!(candidates.len(), 1, "got {candidates:?}");
        assert_eq!(candidates[0].name, "node_modules");
        assert!(matches!(
            candidates[0].kind,
            CandidateKind::RebuildableCache { .. }
        ));
    }

    #[test]
    fn sub_threshold_directories_are_not_nominated() {
        let base = std::env::temp_dir().join("sift-analyze-threshold");
        let mut tree = tree_over(&base);
        let root = tree.root();
        add_dir(&mut tree, root, "node_modules", 1024); // 1 KiB

        let analyzer = Analyzer::new(
            AnalysisPolicy::directory_only().with_min_bytes(1024 * 1024),
            None,
        );
        assert!(analyzer.dir_candidates(&mut tree, 0).is_empty());
    }

    #[test]
    fn file_pass_finds_installers_archives_and_stale_files() {
        let base = std::env::temp_dir().join("sift-analyze-filepass");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let big = 300 * 1024 * 1024;
        std::fs::write(base.join("Installer.dmg"), vec![0u8; big]).unwrap();
        std::fs::write(base.join("backup.zip"), vec![0u8; big]).unwrap();
        std::fs::write(base.join("small.txt"), b"tiny").unwrap();

        let analyzer = Analyzer::new(AnalysisPolicy::default(), None);
        let candidates = analyzer.file_candidates(std::slice::from_ref(&base), 86_400_000 * 400);
        let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Installer.dmg"), "{names:?}");
        assert!(names.contains(&"backup.zip"), "{names:?}");
        assert!(!names.contains(&"small.txt"), "tiny files are ignored");

        let dmg = candidates
            .iter()
            .find(|c| c.name == "Installer.dmg")
            .unwrap();
        assert!(matches!(dmg.kind, CandidateKind::PackageInstaller { .. }));
        let zip = candidates.iter().find(|c| c.name == "backup.zip").unwrap();
        assert!(matches!(zip.kind, CandidateKind::Archive { .. }));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn duplicate_groups_require_the_same_name_and_size() {
        let base = std::env::temp_dir().join("sift-analyze-dupes");
        let _ = std::fs::remove_dir_all(&base);
        let left = base.join("left");
        let right = base.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        let size = 16 * 1024 * 1024;
        // The same name and size in two places: the product's definition of a
        // suspected duplicate (same name + same size), not "same size".
        std::fs::write(left.join("movie.mp4"), vec![0u8; size]).unwrap();
        std::fs::write(right.join("movie.mp4"), vec![0u8; size]).unwrap();
        // A different name at the same size must not be grouped.
        std::fs::write(left.join("other.mp4"), vec![0u8; size]).unwrap();

        let policy = AnalysisPolicy::default().with_min_bytes(1024);
        let analyzer = Analyzer::new(policy, None);
        let candidates = analyzer.file_candidates(&[left.clone(), right.clone()], 0);

        let dupes: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| matches!(c.kind, CandidateKind::DuplicateGroup { .. }))
            .collect();
        assert_eq!(dupes.len(), 2, "exactly the two same-named copies");
        assert!(dupes.iter().all(|c| c.name == "movie.mp4"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn full_pipeline_reports_safe_and_review() {
        let base = std::env::temp_dir().join("sift-analyze-pipeline");
        let mut tree = tree_over(&base);
        let root = tree.root();
        add_dir(&mut tree, root, "node_modules", 600 * 1024 * 1024);
        add_dir(&mut tree, root, "build", 500 * 1024 * 1024);

        let analyzer = Analyzer::new(AnalysisPolicy::directory_only(), None);
        let adjudicator = RuleAdjudicator::new();
        let report = analyzer.analyze(&adjudicator, &mut tree, 0, |_, _, _| {});

        assert_eq!(report.items.len(), 2);
        // node_modules is Safe, "build" is Review: only one is unattended-safe.
        assert_eq!(report.known_cleanable().len(), 1);
        assert_eq!(report.safe_bytes(), 600 * 1024 * 1024);
        assert_eq!(report.reclaimable(), 1100 * 1024 * 1024);
        let sorted = report.sorted_by_size();
        assert_eq!(sorted[0].candidate.name, "node_modules");
        assert_eq!(sorted[0].verdict.safety, Safety::Safe);
        assert_eq!(sorted[1].verdict.safety, Safety::Review);
    }

    #[test]
    fn pipeline_uses_a_verdict_cache_when_supplied() {
        use std::sync::Mutex as StdMutex;

        #[derive(Default)]
        struct MemCache(StdMutex<Vec<(PathFingerprint, crate::reason::Verdict)>>);
        impl VerdictCache for MemCache {
            fn lookup(&self, fingerprint: &PathFingerprint) -> Option<crate::reason::Verdict> {
                self.0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(stored, _)| stored.still_describes(fingerprint))
                    .map(|(_, verdict)| verdict.clone())
            }
            fn store(&self, fingerprint: &PathFingerprint, verdict: &crate::reason::Verdict) {
                self.0.lock().unwrap().push((*fingerprint, verdict.clone()));
            }
        }

        let base = std::env::temp_dir().join("sift-analyze-cache");
        let mut tree = tree_over(&base);
        let root = tree.root();
        add_dir(&mut tree, root, "node_modules", 600 * 1024 * 1024);

        let analyzer = Analyzer::new(AnalysisPolicy::directory_only(), None);
        let cached =
            crate::adjudicate::CachedAdjudicator::new(RuleAdjudicator::new(), MemCache::default());

        let first = analyzer.analyze(&cached, &mut tree, 0, |_, _, _| {});
        assert_eq!(
            first.items[0].verdict.source.label(),
            "rule:dir.node_modules"
        );
        let second = analyzer.analyze(&cached, &mut tree, 1, |_, _, _| {});
        assert_eq!(second.items[0].verdict.source.label(), "cached");
        assert_eq!(cached.stats(), (1, 1));
    }
}
