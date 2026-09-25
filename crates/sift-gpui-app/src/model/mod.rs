//! The workspace model: everything the window shows, as plain data.
//!
//! This module deliberately has **no GPUI dependency**. It consumes the core's
//! events ([`sift_core::ScanEvent`], [`sift_monitor::MonitorEvent`], an
//! [`sift_analyze::AnalysisReport`]) and exposes exactly what a view needs to
//! draw: the current directory's rows, the treemap entries, the candidate list,
//! the selection, and the status line.
//!
//! Two reasons for that split:
//!
//! * **It is testable.** Navigation, incremental size updates, selection and
//!   pruning are verified by ordinary unit tests instead of by driving a window.
//! * **The view stays thin.** The GPUI layer renders this state and forwards
//!   intents; no product rule lives in a render function.
//!
//! The model is also where the product's *display* decisions live: directories
//! before files, biggest first, a pending directory shown as still measuring,
//! and the long tail folded into an "other" tile.

pub mod treemap;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use sift_analyze::{AnalysisReport, Reason, Safety, VerdictSource};
use sift_core::{
    format_bytes, ByteSize, NodeKey, Progress, ScanEvent, ScanOutcome, Volume, VolumeId,
};
use sift_monitor::{AlertLevel, MonitorEvent};

pub use treemap::{layout, Rect, Tile, TreemapEntry, TreemapLayout};

/// One node the window knows about, with its path already resolved by the engine.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct NodeRecord {
    pub key: NodeKey,
    pub parent: Option<NodeKey>,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: ByteSize,
    pub mtime_ms: i64,
    /// A directory still being measured.
    pub pending: bool,
    pub deletable: bool,
}

impl NodeRecord {
    pub fn size_bytes(&self) -> u64 {
        self.size.dominant()
    }

    /// The size to show: an unresolved directory shows a placeholder, not `0 B`.
    pub fn size_label(&self) -> String {
        if self.pending && self.size.is_zero() {
            "…".to_string()
        } else {
            format_bytes(self.size_bytes())
        }
    }
}

/// One analysis conclusion, ready to render.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Finding {
    pub key: NodeKey,
    pub name: String,
    pub path: PathBuf,
    pub display_path: String,
    pub size: u64,
    pub is_dir: bool,
    pub safety: Safety,
    pub confidence: f32,
    pub reason: Reason,
    pub source: VerdictSource,
    /// Candidate family token, e.g. `rebuildableCache`.
    pub kind: String,
    /// Remembered as definitively cleanable.
    pub known_cleanable: bool,
    /// Approved for unattended cleanup.
    pub approved_for_auto: bool,
}

impl Finding {
    /// Why the row cannot be auto-cleaned, or `None` when it can.
    pub fn auto_blocked_reason(&self) -> Option<&'static str> {
        if self.safety != Safety::Safe {
            return Some("finding.blocked.notSafe");
        }
        if !self.approved_for_auto {
            return Some("finding.blocked.notApproved");
        }
        None
    }
}

/// A transient message.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub level: ToastLevel,
    pub created_ms: i64,
}

/// How prominent a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    pub const fn icon_token(self) -> &'static str {
        match self {
            ToastLevel::Info => "info",
            ToastLevel::Success => "check",
            ToastLevel::Warning => "alert",
            ToastLevel::Error => "alert",
        }
    }
}

/// A snapshot of the background monitor, for the status strip.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MonitorSnapshot {
    pub running: bool,
    pub level: AlertLevel,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub reclaimable_bytes: u64,
    pub cleanable_items: usize,
    /// The last cleanup the monitor reported, if any.
    pub last_cleanup: Option<CleanupReport>,
}

impl Default for MonitorSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            level: AlertLevel::Normal,
            available_bytes: 0,
            total_bytes: 0,
            reclaimable_bytes: 0,
            cleanable_items: 0,
            last_cleanup: None,
        }
    }
}

/// The outcome of one monitor-driven cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanupReport {
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes: u64,
}

/// Which surface the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// One volume.
    Volume,
    /// Every mounted volume.
    AllVolumes,
}

/// Everything the window renders.
#[derive(Debug, Default)]
pub struct WorkspaceModel {
    volumes: Vec<Volume>,
    current_volume: Option<VolumeId>,
    surface: Option<Surface>,

    nodes: HashMap<NodeKey, NodeRecord>,
    children: HashMap<NodeKey, Vec<NodeKey>>,
    root: Option<NodeKey>,
    current_dir: Option<NodeKey>,

    scanning: bool,
    progress: Progress,
    outcome: Option<ScanOutcome>,

    /// The tile the pointer or keyboard is on, used for the linked highlight
    /// between the treemap and the list.
    focus_key: Option<NodeKey>,
    selected: BTreeSet<NodeKey>,
    drawer_open: bool,

    findings: Vec<Finding>,
    analyzing: bool,

    monitor: MonitorSnapshot,
    toasts: Vec<Toast>,
    next_toast_id: u64,
}

impl WorkspaceModel {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- volumes -----------------------------------------------------------

    pub fn set_volumes(&mut self, volumes: Vec<Volume>) {
        self.volumes = volumes;
    }

    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    pub fn current_volume(&self) -> Option<&Volume> {
        let id = self.current_volume?;
        self.volumes.iter().find(|volume| volume.id == id)
    }

    pub fn current_volume_id(&self) -> Option<VolumeId> {
        self.current_volume
    }

    pub fn surface(&self) -> Option<Surface> {
        self.surface
    }

    /// Select a volume and clear the previous tree.
    pub fn select_volume(&mut self, id: VolumeId) {
        self.current_volume = Some(id);
        self.surface = Some(Surface::Volume);
        self.clear_tree();
    }

    /// Show every mounted volume.
    pub fn show_all_volumes(&mut self) {
        self.surface = Some(Surface::AllVolumes);
        self.current_volume = None;
        self.clear_tree();
    }

    fn clear_tree(&mut self) {
        self.nodes.clear();
        self.children.clear();
        self.root = None;
        self.current_dir = None;
        self.findings.clear();
        self.selected.clear();
        self.focus_key = None;
        self.progress = Progress::default();
        self.outcome = None;
    }

    // ---- scanning ----------------------------------------------------------

    /// Mark a scan as started for the current volume.
    pub fn begin_scan(&mut self) {
        self.clear_tree();
        self.scanning = true;
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub fn scan_outcome(&self) -> Option<ScanOutcome> {
        self.outcome
    }

    pub fn progress(&self) -> Progress {
        self.progress
    }

    /// Apply one engine event.
    ///
    /// Events from a scan of a different volume are ignored: the model clears
    /// its tree on volume change, and a late event from the previous scan must
    /// not resurrect it.
    pub fn apply_scan_event(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::DirectoryListed { dir, .. } => {
                let dir_key = dir.key;
                let dir_path = PathBuf::from(&dir.path);
                // The directory's own record (the root arrives only here).
                let parent = self.infer_parent(dir_key);
                self.nodes.insert(
                    dir_key,
                    NodeRecord {
                        key: dir_key,
                        parent,
                        name: dir_path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| dir.path.clone()),
                        path: dir_path.clone(),
                        is_dir: true,
                        size: dir.size,
                        mtime_ms: 0,
                        pending: true,
                        deletable: false,
                    },
                );
                if self.root.is_none() {
                    self.root = Some(dir_key);
                    self.current_dir = Some(dir_key);
                }

                let mut child_keys = Vec::with_capacity(dir.entries.len());
                for entry in &dir.entries {
                    let path = join(&dir_path, &entry.name);
                    let child = NodeRecord {
                        key: entry.key,
                        parent: Some(dir_key),
                        name: entry.name.clone(),
                        path,
                        is_dir: entry.is_dir,
                        size: entry.size,
                        mtime_ms: entry.mtime_ms,
                        pending: entry.pending,
                        deletable: entry.deletable,
                    };
                    child_keys.push(entry.key);
                    self.nodes.insert(entry.key, child);
                }
                self.children.insert(dir_key, child_keys);
            }
            ScanEvent::DirectorySized {
                key, size, pending, ..
            } => {
                if let Some(node) = self.nodes.get_mut(&key) {
                    node.size = size;
                    node.pending = pending;
                }
            }
            ScanEvent::DirectoryClosed { key, .. } => {
                if let Some(node) = self.nodes.get_mut(&key) {
                    node.pending = false;
                }
            }
            ScanEvent::Progress { progress, .. } => {
                self.progress = progress;
            }
            ScanEvent::Finished { outcome, .. } => {
                self.scanning = false;
                self.outcome = Some(outcome);
                // Nothing is still measuring once the scan has stopped: a
                // cancelled scan must not leave rows claiming to be pending.
                for node in self.nodes.values_mut() {
                    node.pending = false;
                }
            }
            ScanEvent::Warning { message, .. } => {
                self.toast(message, ToastLevel::Warning, 0);
            }
            // Non-exhaustive: an unknown future event changes nothing.
            _ => {}
        }
    }

    /// The parent of a directory that has not been seen as a child yet.
    ///
    /// A `DirectoryListed` for a subdirectory always follows the listing of its
    /// parent, so the parent link is recoverable from the path.
    fn infer_parent(&self, key: NodeKey) -> Option<NodeKey> {
        // Already known (a re-listing): keep the existing link.
        if let Some(existing) = self.nodes.get(&key) {
            return existing.parent;
        }
        None
    }

    /// Record a child's parent after its own listing established the link.
    pub fn note_parent(&mut self, child: NodeKey, parent: NodeKey) {
        if let Some(node) = self.nodes.get_mut(&child) {
            node.parent = Some(parent);
        }
    }

    // ---- navigation --------------------------------------------------------

    pub fn current_dir(&self) -> Option<NodeKey> {
        self.current_dir
    }

    pub fn current_node(&self) -> Option<&NodeRecord> {
        self.current_dir.and_then(|key| self.nodes.get(&key))
    }

    pub fn node(&self, key: NodeKey) -> Option<&NodeRecord> {
        self.nodes.get(&key)
    }

    pub fn set_focus(&mut self, key: Option<NodeKey>) {
        self.focus_key = key;
    }

    pub fn focus_key(&self) -> Option<NodeKey> {
        self.focus_key
    }

    /// Move into a directory.
    pub fn drill_into(&mut self, key: NodeKey) -> bool {
        match self.nodes.get(&key) {
            Some(node) if node.is_dir => {
                self.current_dir = Some(key);
                self.focus_key = None;
                true
            }
            _ => false,
        }
    }

    /// The ancestor chain from the root down to the current directory.
    pub fn breadcrumbs(&self) -> Vec<&NodeRecord> {
        let mut chain: Vec<&NodeRecord> = Vec::new();
        let mut cursor = self.current_dir;
        while let Some(key) = cursor {
            let Some(node) = self.nodes.get(&key) else { break };
            chain.push(node);
            cursor = node.parent;
        }
        chain.reverse();
        chain
    }

    /// Direct children of the current directory: directories first, then by
    /// size descending, with a stable tie-break so rows never reshuffle.
    pub fn visible_entries(&self) -> Vec<&NodeRecord> {
        let Some(dir) = self.current_dir else {
            return Vec::new();
        };
        let Some(keys) = self.children.get(&dir) else {
            return Vec::new();
        };
        let mut nodes: Vec<&NodeRecord> = keys.iter().filter_map(|key| self.nodes.get(key)).collect();
        nodes.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| right.size_bytes().cmp(&left.size_bytes()))
                .then_with(|| left.key.cmp(&right.key))
        });
        nodes
    }

    /// Entries for the treemap, largest first.
    pub fn treemap_entries(&self) -> Vec<TreemapEntry> {
        let mut entries: Vec<TreemapEntry> = self
            .visible_entries()
            .into_iter()
            .filter(|node| node.size_bytes() > 0)
            .map(|node| TreemapEntry {
                key: node.key,
                label: node.name.clone(),
                size: node.size_bytes(),
                is_dir: node.is_dir,
            })
            .collect();
        entries.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.key.cmp(&right.key))
        });
        entries
    }

    /// Lay the treemap out for a stage of `area`, folding the long tail.
    pub fn treemap_layout(&self, area: Rect, min_tile_area: f32) -> TreemapLayout {
        layout(&self.treemap_entries(), area, min_tile_area)
    }

    /// Total bytes of the current directory, as the treemap's base.
    pub fn current_total(&self) -> u64 {
        self.current_node().map(|node| node.size_bytes()).unwrap_or(0)
    }

    // ---- selection ---------------------------------------------------------

    pub fn toggle_selected(&mut self, key: NodeKey) {
        let Some(node) = self.nodes.get(&key) else {
            return;
        };
        if !node.deletable {
            // A row that cannot be deleted must not silently enter the queue;
            // the view explains why with `refusal`.
            return;
        }
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
    }

    pub fn is_selected(&self, key: NodeKey) -> bool {
        self.selected.contains(&key)
    }

    pub fn selection(&self) -> &BTreeSet<NodeKey> {
        &self.selected
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// The selection, biggest first.
    pub fn selected_items(&self) -> Vec<&NodeRecord> {
        let mut items: Vec<&NodeRecord> = self
            .selected
            .iter()
            .filter_map(|key| self.nodes.get(key))
            .collect();
        items.sort_by(|left, right| {
            right
                .size_bytes()
                .cmp(&left.size_bytes())
                .then_with(|| left.key.cmp(&right.key))
        });
        items
    }

    pub fn selected_bytes(&self) -> u64 {
        self.selected_items()
            .iter()
            .map(|node| node.size.reclaimable())
            .sum()
    }

    pub fn drawer_open(&self) -> bool {
        self.drawer_open
    }

    pub fn set_drawer_open(&mut self, open: bool) {
        self.drawer_open = open;
    }

    // ---- external deletion -------------------------------------------------

    /// Remove a subtree after it disappeared outside the app.
    ///
    /// Returns whether anything changed, so the caller can avoid a redraw.
    pub fn prune_subtree(&mut self, key: NodeKey) -> bool {
        // Remember where focus should land before the node disappears.
        let Some(survivor) = self.nodes.get(&key).map(|node| node.parent) else {
            return false;
        };
        let mut doomed: HashSet<NodeKey> = HashSet::new();
        let mut stack = vec![key];
        while let Some(current) = stack.pop() {
            if !doomed.insert(current) {
                continue;
            }
            if let Some(children) = self.children.get(&current) {
                stack.extend(children.iter().copied());
            }
        }
        for key in &doomed {
            self.nodes.remove(key);
            self.children.remove(key);
            self.selected.remove(key);
            self.findings.retain(|finding| finding.key != *key);
            if self.focus_key == Some(*key) {
                self.focus_key = None;
            }
        }
        // Re-point the current directory at the nearest surviving ancestor.
        let root_doomed = self.root.is_some_and(|root| doomed.contains(&root));
        if root_doomed {
            self.root = None;
            self.current_dir = None;
        } else if self.current_dir.is_some_and(|current| doomed.contains(&current)) {
            self.current_dir = survivor.or(self.root);
        }
        true
    }

    // ---- analysis ----------------------------------------------------------

    pub fn begin_analysis(&mut self) {
        self.analyzing = true;
    }

    pub fn is_analyzing(&self) -> bool {
        self.analyzing
    }

    /// Replace the findings with a fresh analysis run.
    pub fn apply_analysis(&mut self, report: &AnalysisReport, approved: &HashSet<NodeKey>) {
        self.analyzing = false;
        let mut findings: Vec<Finding> = report
            .items
            .iter()
            .map(|item| Finding {
                key: item.candidate.key,
                name: item.candidate.name.clone(),
                path: item.candidate.path.clone(),
                display_path: item.candidate.display_path.clone(),
                size: item.candidate.reclaimable(),
                is_dir: item.candidate.is_dir,
                safety: item.verdict.safety,
                confidence: item.verdict.confidence,
                reason: item.verdict.reason.clone(),
                source: item.verdict.source.clone(),
                kind: item.candidate.kind.token().to_string(),
                known_cleanable: item.verdict.safety == Safety::Safe,
                approved_for_auto: approved.contains(&item.candidate.key),
            })
            .collect();
        findings.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.key.cmp(&right.key))
        });
        self.findings = findings;
    }

    pub fn fail_analysis(&mut self, message: impl Into<String>, now_ms: i64) {
        self.analyzing = false;
        self.toast(message, ToastLevel::Error, now_ms);
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Findings the user may act on, biggest first.
    pub fn removable_findings(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.safety.is_removable())
            .collect()
    }

    /// Bytes reclaimable if everything removable went away.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.removable_findings()
            .iter()
            .map(|finding| finding.size)
            .sum()
    }

    /// Bytes reclaimable with no user decision.
    pub fn safe_bytes(&self) -> u64 {
        self.findings
            .iter()
            .filter(|finding| finding.safety.is_automatic())
            .map(|finding| finding.size)
            .sum()
    }

    pub fn mark_approved(&mut self, key: NodeKey, approved: bool) {
        if let Some(finding) = self.findings.iter_mut().find(|item| item.key == key) {
            finding.approved_for_auto = approved;
        }
    }

    // ---- monitor -----------------------------------------------------------

    pub fn monitor(&self) -> &MonitorSnapshot {
        &self.monitor
    }

    pub fn set_monitor_running(&mut self, running: bool) {
        self.monitor.running = running;
    }

    pub fn set_monitor_snapshot(
        &mut self,
        level: AlertLevel,
        available: u64,
        total: u64,
        reclaimable: u64,
        cleanable_items: usize,
    ) {
        self.monitor.level = level;
        self.monitor.available_bytes = available;
        self.monitor.total_bytes = total;
        self.monitor.reclaimable_bytes = reclaimable;
        self.monitor.cleanable_items = cleanable_items;
    }

    /// Apply one monitor event. `now_ms` stamps any toast it produces.
    pub fn apply_monitor_event(&mut self, event: &MonitorEvent, now_ms: i64) {
        match event {
            MonitorEvent::Sampled {
                level,
                sample,
                reclaimable_bytes,
                entry_count,
            } => {
                self.set_monitor_snapshot(
                    *level,
                    sample.available_bytes,
                    sample.total_bytes,
                    *reclaimable_bytes,
                    *entry_count,
                );
            }
            MonitorEvent::Notify {
                level,
                available_bytes,
                reclaimable_bytes,
                entry_count,
            } => {
                self.monitor.level = *level;
                self.monitor.available_bytes = *available_bytes;
                let message = format!(
                    "磁盘剩余 {}，可清理约 {}（{} 项）",
                    format_bytes(*available_bytes),
                    format_bytes(*reclaimable_bytes),
                    entry_count
                );
                self.toast(message, ToastLevel::Warning, now_ms);
            }
            MonitorEvent::ConfirmationNeeded {
                level,
                reclaimable_bytes,
                entry_count,
                ..
            } => {
                self.monitor.level = *level;
                let message = format!(
                    "可清理 {}（{} 项），需要你确认",
                    format_bytes(*reclaimable_bytes),
                    entry_count
                );
                self.toast(message, ToastLevel::Info, now_ms);
            }
            MonitorEvent::CleanupStarted { entry_count, bytes } => {
                let message = format!(
                    "正在自动清理 {} 项，约 {}",
                    entry_count,
                    format_bytes(*bytes)
                );
                self.toast(message, ToastLevel::Info, now_ms);
            }
            MonitorEvent::CleanupFinished {
                removed,
                skipped,
                failed,
                bytes,
            } => {
                self.monitor.last_cleanup = Some(CleanupReport {
                    removed: *removed,
                    skipped: *skipped,
                    failed: *failed,
                    bytes: *bytes,
                });
                let message = format!(
                    "已释放 {}（{} 项，跳过 {}，失败 {}）",
                    format_bytes(*bytes),
                    removed,
                    skipped,
                    failed
                );
                let level = if *failed > 0 {
                    ToastLevel::Warning
                } else {
                    ToastLevel::Success
                };
                self.toast(message, level, now_ms);
            }
            MonitorEvent::Warning(message) => {
                self.toast(message.clone(), ToastLevel::Warning, now_ms);
            }
            _ => {}
        }
    }

    // ---- toasts ------------------------------------------------------------

    pub fn toast(&mut self, message: impl Into<String>, level: ToastLevel, now_ms: i64) {
        self.next_toast_id += 1;
        self.toasts.push(Toast {
            id: self.next_toast_id,
            message: message.into(),
            level,
            created_ms: now_ms,
        });
        // Keep the stack short: an unbounded toast list is a memory leak with a
        // friendly name.
        if self.toasts.len() > 4 {
            let excess = self.toasts.len() - 4;
            self.toasts.drain(..excess);
        }
    }

    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Drop toasts older than `ttl_ms`.
    pub fn expire_toasts(&mut self, now_ms: i64, ttl_ms: i64) -> bool {
        let before = self.toasts.len();
        self.toasts
            .retain(|toast| now_ms.saturating_sub(toast.created_ms) < ttl_ms);
        self.toasts.len() != before
    }

    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    // ---- pruning -----------------------------------------------------------

    /// Drop remembered verdicts for paths that no longer exist.
    pub fn live_keys(&self) -> HashSet<NodeKey> {
        self.nodes.keys().copied().collect()
    }

    /// A one-line status for the footer.
    pub fn status_line(&self) -> String {
        if self.scanning {
            return format!(
                "正在扫描：{} 文件 · {} 文件夹",
                self.progress.files, self.progress.dirs
            );
        }
        if !self.selected.is_empty() {
            return format!(
                "待清理 {} · {} 项",
                format_bytes(self.selected_bytes()),
                self.selected.len()
            );
        }
        if let Some(volume) = self.current_volume() {
            return format!("{} 可用", format_bytes(volume.available_bytes));
        }
        let unknown = "—";
        unknown.to_string()
    }
}

/// Join a directory path and an entry name.
pub fn join(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    path.push(name);
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::{DirectoryEntry, DirectorySummary, ScanId};

    const GB: u64 = 1024 * 1024 * 1024;

    /// Keys are path-derived everywhere, exactly as the engine computes them:
    /// a child listed by its parent has the *same* key as that path's own
    /// directory listing. Fixtures that invent keys would hide real bugs.
    fn key(path: &str) -> NodeKey {
        NodeKey::from_path(Path::new(path))
    }

    fn name_of(path: &str) -> String {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    }

    fn entry(path: &str, size: u64, is_dir: bool, pending: bool) -> DirectoryEntry {
        DirectoryEntry::new(
            key(path),
            name_of(path),
            is_dir,
            ByteSize::new(size, size),
            100,
            pending,
            true,
        )
    }

    fn locked_entry(path: &str, size: u64) -> DirectoryEntry {
        DirectoryEntry::new(
            key(path),
            name_of(path),
            false,
            ByteSize::new(size, size),
            100,
            false,
            false,
        )
    }

    fn listed(dir_path: &str, entries: Vec<DirectoryEntry>) -> ScanEvent {
        ScanEvent::DirectoryListed {
            scan: ScanId(1),
            dir: DirectorySummary::new(
                key(dir_path),
                dir_path,
                ByteSize::new(entries.iter().map(|e| e.size.logical).sum(), 0),
                entries.len() as u32,
                entries.iter().filter(|e| e.is_dir).count() as u32,
                entries,
            ),
        }
    }

    fn finished() -> ScanEvent {
        ScanEvent::Finished {
            scan: ScanId(1),
            outcome: ScanOutcome::Completed,
            progress: Progress::default(),
        }
    }

    #[test]
    fn a_listed_directory_becomes_rows() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        assert!(model.is_scanning());
        model.apply_scan_event(listed(
            "/root",
            vec![
                entry("/root/a.txt", 300, false, false),
                entry("/root/sub", 200, true, true),
            ],
        ));

        assert_eq!(model.current_dir(), Some(key("/root")));
        let rows = model.visible_entries();
        assert_eq!(rows.len(), 2);
        // Directories first, then by size.
        assert_eq!(rows[0].name, "sub");
        assert_eq!(rows[1].name, "a.txt");
        assert_eq!(rows[1].path, PathBuf::from("/root/a.txt"));
        assert_eq!(model.current_total(), 500);
    }

    #[test]
    fn sizes_update_incrementally_and_pending_clears_on_finish() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/sub", 0, true, true)]));
        assert!(model.node(key("/sub")).unwrap().pending);

        model.apply_scan_event(ScanEvent::DirectorySized {
            scan: ScanId(1),
            key: key("/sub"),
            size: ByteSize::new(5 * GB, 5 * GB),
            files: 12,
            pending: true,
        });
        assert_eq!(model.node(key("/sub")).unwrap().size_bytes(), 5 * GB);

        model.apply_scan_event(ScanEvent::DirectoryClosed {
            scan: ScanId(1),
            key: key("/sub"),
        });
        assert!(!model.node(key("/sub")).unwrap().pending);

        // A cancelled scan must not leave anything claiming to be measuring.
        model.apply_scan_event(listed("/", vec![entry("/other", 0, true, true)]));
        model.apply_scan_event(ScanEvent::Finished {
            scan: ScanId(1),
            outcome: ScanOutcome::Cancelled,
            progress: Progress::default(),
        });
        assert!(!model.is_scanning());
        assert_eq!(model.scan_outcome(), Some(ScanOutcome::Cancelled));
        assert!(!model.node(key("/other")).unwrap().pending);
    }

    #[test]
    fn unresolved_directories_show_a_placeholder_not_zero() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/sub", 0, true, true)]));
        assert_eq!(model.node(key("/sub")).unwrap().size_label(), "…");

        model.apply_scan_event(ScanEvent::DirectorySized {
            scan: ScanId(1),
            key: key("/sub"),
            size: ByteSize::new(1536, 0),
            files: 1,
            pending: false,
        });
        assert_eq!(model.node(key("/sub")).unwrap().size_label(), "1.5 KB");
    }

    #[test]
    fn navigation_walks_a_breadcrumb_chain() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/Users", 0, true, true)]));
        // The parent link comes from the listing, exactly as the engine provides.
        assert_eq!(model.node(key("/Users")).unwrap().parent, Some(key("/")));

        assert!(model.drill_into(key("/Users")));
        model.apply_scan_event(listed(
            "/Users",
            vec![entry("/Users/me", 0, true, true)],
        ));
        assert_eq!(model.node(key("/Users/me")).unwrap().parent, Some(key("/Users")));
        assert!(model.drill_into(key("/Users/me")));

        let crumbs: Vec<&str> = model
            .breadcrumbs()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(crumbs, vec!["/", "Users", "me"]);

        // Drilling into a file is refused.
        model.apply_scan_event(listed(
            "/Users/me",
            vec![entry("/Users/me/notes.txt", 10, false, false)],
        ));
        assert!(!model.drill_into(key("/Users/me/notes.txt")));
        assert_eq!(model.current_dir(), Some(key("/Users/me")));
        model.apply_scan_event(finished());
    }

    #[test]
    fn treemap_entries_exclude_empty_and_sort_by_size() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed(
            "/",
            vec![
                entry("/small", 10, false, false),
                entry("/empty", 0, false, false),
                entry("/big", 1000, false, false),
            ],
        ));
        let entries = model.treemap_entries();
        assert_eq!(entries.len(), 2, "a zero-byte entry has no tile");
        assert_eq!(entries[0].label, "big");
        assert_eq!(model.current_total(), 1010);
    }

    #[test]
    fn the_treemap_folds_a_long_tail_and_reports_it() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        let mut entries = vec![entry("/big", 400 * 1024 * 1024, true, false)];
        for index in 0..50u64 {
            entries.push(entry(&format!("/small-{index}"), 1024, false, false));
        }
        model.apply_scan_event(listed("/", entries));

        let laid = model.treemap_layout(Rect::new(0.0, 0.0, 800.0, 600.0), 3000.0);
        assert!(laid.tiles.iter().any(|tile| tile.is_aggregate()));
        assert_eq!(laid.folded.len(), 50);
        assert_eq!(laid.folded_bytes, 50 * 1024);
    }

    #[test]
    fn selection_ignores_undeletable_rows_and_sums_bytes() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed(
            "/",
            vec![
                entry("/a", 300, false, false),
                locked_entry("/locked", 100),
                entry("/b", 200, false, false),
            ],
        ));

        model.toggle_selected(key("/locked"));
        assert!(model.selection().is_empty(), "a locked row cannot be queued");

        model.toggle_selected(key("/a"));
        model.toggle_selected(key("/b"));
        assert_eq!(model.selected_bytes(), 500);
        let names: Vec<&str> = model
            .selected_items()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"], "biggest first");

        // Toggling again removes it.
        model.toggle_selected(key("/a"));
        assert_eq!(model.selected_bytes(), 200);
        model.clear_selection();
        assert!(model.selection().is_empty());
    }

    #[test]
    fn a_subtree_that_vanishes_is_pruned() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/Users", 0, true, true)]));
        model.apply_scan_event(listed(
            "/Users",
            vec![
                entry("/Users/me", 0, true, true),
                entry("/Users/other", 0, true, true),
            ],
        ));
        model.apply_scan_event(listed(
            "/Users/me",
            vec![entry("/Users/me/notes.txt", 10, false, false)],
        ));

        assert!(model.drill_into(key("/Users/me")));
        model.toggle_selected(key("/Users/me/notes.txt"));
        assert!(model.prune_subtree(key("/Users/me")));

        assert!(model.node(key("/Users/me")).is_none());
        assert!(
            model.node(key("/Users/me/notes.txt")).is_none(),
            "the subtree goes too"
        );
        assert!(model.selection().is_empty());
        assert_eq!(model.current_dir(), Some(key("/Users")), "focus moves up");
        assert!(model.node(key("/Users/other")).is_some(), "siblings survive");
        assert!(!model.prune_subtree(key("/nonexistent")));
    }

    fn candidate_for(path: &str, size: u64, rule: &str, tool: &str) -> sift_analyze::Candidate {
        let mut candidate = sift_analyze::Candidate::new(
            key(path),
            PathBuf::from(path),
            format!("~{path}"),
            name_of(path),
            true,
            ByteSize::new(size, size),
            0,
            sift_analyze::CandidateKind::RebuildableCache {
                tool: tool.to_string(),
            },
        );
        candidate
            .evidence
            .push(sift_analyze::Evidence::new(rule, "detail.patternMatch"));
        // A verdict is a restatement of the nomination, so a fixture without one
        // is correctly `Review`. These fixtures stand in for real rule output.
        let safety = if rule == "dir.node_modules" {
            sift_analyze::Safety::Safe
        } else {
            sift_analyze::Safety::Review
        };
        candidate.with_nomination(sift_analyze::Nomination {
            rule: rule.to_string(),
            safety,
            reason_key: "reason.rebuildableCache".to_string(),
            reason_param: Some(tool.to_string()),
            detail_key: "detail.patternMatch".to_string(),
        })
    }

    #[test]
    fn findings_split_removable_from_automatic() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed(
            "/",
            vec![
                entry("/node_modules", 4 * GB, true, false),
                entry("/build", 2 * GB, true, false),
            ],
        ));

        // The verdict must be derived from the candidate that carries the rule
        // evidence, which is what makes it Safe rather than an unjudged Review.
        let safe = candidate_for("/node_modules", 4 * GB, "dir.node_modules", "npm");
        let review = candidate_for("/build", 2 * GB, "dir.rust_gradle_build", "build");
        let report = AnalysisReport::new(
            vec![
                sift_analyze::AnalyzedItem::new(
                    safe.clone(),
                    sift_analyze::adjudicate::rule_verdict(&safe, 0),
                ),
                sift_analyze::AnalyzedItem::new(
                    review.clone(),
                    sift_analyze::adjudicate::rule_verdict(&review, 0),
                ),
            ],
            0,
            1,
        );

        model.begin_analysis();
        assert!(model.is_analyzing());
        model.apply_analysis(&report, &HashSet::new());
        assert!(!model.is_analyzing());

        assert_eq!(model.findings().len(), 2);
        assert_eq!(model.findings()[0].name, "node_modules", "biggest first");
        assert_eq!(model.safe_bytes(), 4 * GB);
        assert_eq!(model.reclaimable_bytes(), 6 * GB);
        assert!(model.findings()[0].known_cleanable);
        assert_eq!(
            model.findings()[0].auto_blocked_reason(),
            Some("finding.blocked.notApproved")
        );
        assert_eq!(
            model.findings()[1].auto_blocked_reason(),
            Some("finding.blocked.notSafe")
        );

        model.mark_approved(key("/node_modules"), true);
        assert!(model.findings()[0].approved_for_auto);
        assert_eq!(model.findings()[0].auto_blocked_reason(), None);

        // A fresh analysis carries the approval set it is given.
        let approved: HashSet<NodeKey> = [key("/node_modules")].into_iter().collect();
        model.apply_analysis(&report, &approved);
        assert!(model.findings()[0].approved_for_auto);
    }

    #[test]
    fn a_failed_analysis_reports_instead_of_hanging() {
        let mut model = WorkspaceModel::new();
        model.begin_analysis();
        model.fail_analysis("模型不可用", 5);
        assert!(!model.is_analyzing());
        assert_eq!(model.toasts().len(), 1);
        assert_eq!(model.toasts()[0].level, ToastLevel::Error);
    }

    #[test]
    fn monitor_events_drive_the_status_and_toasts() {
        let mut model = WorkspaceModel::new();
        model.apply_monitor_event(
            &MonitorEvent::Sampled {
                level: AlertLevel::Warning,
                sample: sift_monitor::DiskSample::new(1000 * GB, 100 * GB, 0),
                reclaimable_bytes: 30 * GB,
                entry_count: 3,
            },
            0,
        );
        assert_eq!(model.monitor().level, AlertLevel::Warning);
        assert_eq!(model.monitor().available_bytes, 100 * GB);
        assert!(model.toasts().is_empty(), "a routine reading is silent");

        model.apply_monitor_event(
            &MonitorEvent::CleanupFinished {
                removed: 2,
                skipped: 1,
                failed: 0,
                bytes: 30 * GB,
            },
            10,
        );
        assert_eq!(model.toasts().len(), 1);
        assert_eq!(model.toasts()[0].level, ToastLevel::Success);
        let report = model.monitor().last_cleanup.expect("a report");
        assert_eq!(report.removed, 2);
        assert_eq!(report.skipped, 1);

        // Failures are reported with a warning level.
        model.apply_monitor_event(
            &MonitorEvent::CleanupFinished {
                removed: 0,
                skipped: 0,
                failed: 1,
                bytes: 0,
            },
            20,
        );
        assert_eq!(model.toasts().last().unwrap().level, ToastLevel::Warning);
    }

    #[test]
    fn a_threshold_notification_names_the_numbers() {
        let mut model = WorkspaceModel::new();
        model.apply_monitor_event(
            &MonitorEvent::Notify {
                level: AlertLevel::Critical,
                available_bytes: 2 * GB,
                reclaimable_bytes: 40 * GB,
                entry_count: 5,
            },
            0,
        );
        assert_eq!(model.monitor().level, AlertLevel::Critical);
        let message = &model.toasts()[0].message;
        assert!(message.contains("2.0 GB"), "{message}");
        assert!(message.contains("40.0 GB"), "{message}");
        assert!(message.contains('5'), "{message}");
    }

    #[test]
    fn toasts_expire_and_the_stack_stays_short() {
        let mut model = WorkspaceModel::new();
        for index in 0..10 {
            model.toast(format!("message {index}"), ToastLevel::Info, index);
        }
        assert_eq!(model.toasts().len(), 4, "the stack is capped");
        model.dismiss_toast(model.toasts()[0].id);
        assert_eq!(model.toasts().len(), 3);

        // Expiry uses the timestamp it is given, not a clock read.
        assert!(model.expire_toasts(50_000, 1000));
        assert!(model.toasts().is_empty());
        assert!(!model.expire_toasts(50_000, 1000), "nothing left to expire");
    }

    #[test]
    fn status_line_follows_the_state() {
        let mut model = WorkspaceModel::new();
        assert_eq!(model.status_line(), "—");

        let volume = Volume::new(
            VolumeId::from_mount_point(Path::new("/")),
            "Macintosh HD",
            "/",
            1000 * GB,
            400 * GB,
            false,
            "apfs",
        );
        model.set_volumes(vec![volume.clone()]);
        model.select_volume(volume.id);
        assert!(model.status_line().contains("400 GB"));

        model.begin_scan();
        let mut progress = Progress::default();
        progress.files = 1200;
        progress.dirs = 40;
        model.apply_scan_event(ScanEvent::Progress {
            scan: ScanId(1),
            progress,
        });
        assert!(model.status_line().contains("1200"), "{}", model.status_line());

        model.apply_scan_event(listed("/", vec![entry("/a", 500, false, false)]));
        model.apply_scan_event(finished());
        model.toggle_selected(key("/a"));
        assert!(
            model.status_line().contains("待清理"),
            "{}",
            model.status_line()
        );
    }

    #[test]
    fn changing_volume_clears_the_previous_tree() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/a", 1, false, false)]));
        assert!(model.node(key("/")).is_some());

        model.select_volume(VolumeId::from_mount_point(Path::new("/Volumes/Other")));
        assert!(model.node(key("/")).is_none(), "the old tree must not linger");
        assert_eq!(model.current_dir(), None);
        assert_eq!(model.surface(), Some(Surface::Volume));

        model.show_all_volumes();
        assert_eq!(model.surface(), Some(Surface::AllVolumes));
        assert!(model.current_volume().is_none());
    }

    #[test]
    fn a_warning_event_becomes_a_toast() {
        let mut model = WorkspaceModel::new();
        model.apply_scan_event(ScanEvent::Warning {
            scan: ScanId(1),
            message: "hardlink table full".into(),
        });
        assert_eq!(model.toasts().len(), 1);
        assert_eq!(model.toasts()[0].level, ToastLevel::Warning);
    }

    #[test]
    fn live_keys_are_just_the_known_nodes() {
        let mut model = WorkspaceModel::new();
        model.begin_scan();
        model.apply_scan_event(listed("/", vec![entry("/a", 1, false, false)]));
        let live = model.live_keys();
        assert_eq!(live.len(), 2);
        assert!(live.contains(&key("/")));
        assert!(live.contains(&key("/a")));
    }
}
