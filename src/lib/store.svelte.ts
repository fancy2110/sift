import { pickDirectory } from './dialog';
import { isTauri } from './runtime';
import {
  analyzeCurrent,
  cancelScan,
  cleanPaths,
  getAiConfig,
  saveAiConfig,
  deleteRoutine,
  dismissRoutineSuggestion,
  listHistory,
  listDirFiles,
  listRoutines,
  listVolumes,
  markPath,
  monitorStatus,
  onFsDeleted,
  onMonitorEvent,
  onScanBatch,
  onScanDone,
  onScanRefreshed,
  resolvePermission,
  routineSuggestions,
  runRoutine,
  saveRoutine,
  scanRunning,
  setAutoCleanMode,
  setScheduledCleanup,
  setScanFocus,
  startMonitor,
  startScan,
  takeStoreWarnings,
  toggleRoutineMode,
  watchFs,
  acceptRoutineSuggestion,
  type BatchUpdate,
  type DeleteResultItem,
  type PermissionRequest,
  type ProgressInfo,
  type RefreshedEvent,
  type ScanDoneEvent
} from './ipc';
import { findSurvivingAncestor, prunedSizeDeltas } from './tree-prune';
import type {
  AiConfig,
  Finding,
  HistoryEntry,
  MonitorStatus as MonitorStatusInfo,
  Node,
  Routine,
  RoutineSuggestion,
  VolumeInfo
} from './types';
import { t, tr, renderStoreWarning } from './i18n.svelte';

export interface Toast {
  id: number;
  message: string;
}

/**
 * Live, event-driven state behind the three-view hub (home / dashboard / sub).
 * Nodes stream in through batched scan updates; the explorer renders the
 * current folder as soon as its first batch arrives. Candidates, history and
 * routines all come from the backend so the same guardrails run everywhere.
 */
class AppStore {
  // ---- hub navigation ----
  view = $state<'home' | 'dashboard' | 'sub'>('home');
  subTab = $state<'smart' | 'explorer' | 'history' | 'routines'>('smart');
  settingsOpen = $state(false);
  /** AI provider configuration behind the settings sheet. */
  aiConfig = $state<AiConfig | null>(null);

  // ---- scan scopes ----
  volumes = $state<VolumeInfo[]>([]);
  scopeId = $state<string | null>(null);

  /**
   * Directory records of the active scan in plain, non-reactive maps. A whole
   * volume holds well over a million directories; wrapping every record in a
   * deep $state proxy costs gigabytes. Only the open folder's entries are
   * exposed reactively (`listEntries`); other records are looked up on demand.
   */
  nodeRecords = new Map<string, Node>();
  /** Directory id -> ids of its directory children (plain index). */
  childIds = new Map<string, string[]>();
  /** Path -> id, so external events (auto-clean finished) resolve in O(1). */
  pathToId = new Map<string, string>();
  rootId: string | null = null;
  // Reactive: the explorer crumbs are derived from this id. A plain field
  // would leave drillPath/breadcrumbs stale after a drill because Svelte's
  // fine-grained updates only re-run expressions that read some $state.
  currentNodeId = $state<string | null>(null);
  /** File entries of the open folder (fetched live; merged into listEntries). */
  currentFiles: Node[] = [];
  /** Reactive slice: entries of the open folder, folders first then size. */
  listEntries = $state<Node[]>([]);

  scanning = $state(false);
  scannedFiles = $state(0);
  scannedDirs = $state(0);

  /** Live counters behind the home progress ring. */
  progress = $state<ProgressInfo>({
    files: 0,
    dirs: 0,
    bytes: 0,
    totalBytes: 0,
    percent: 0,
    rateBytesPerSec: 0,
    awaiting: 0,
    trackedNodes: 0
  });
  /** Directories parked pending a macOS authorization decision. */
  permissionRequests = $state<PermissionRequest[]>([]);

  toasts = $state<Toast[]>([]);
  autoOn = $state(false);
  monitorRunning = $state(false);
  scheduledOn = $state(false);
  scheduledHour = $state(4);

  /** Analyzed candidates from the backend. */
  findings = $state<Finding[]>([]);

  /**
   * Node id -> finding lookup, rebuilt whenever findings change. This is the
   * single source of truth for a row's marked state: annotations are read
   * through it instead of being copied onto node snapshots, where a live
   * re-fetch of the open folder could overwrite them.
   */
  findingsById = $derived.by(() => {
    const map = new Map<string, Finding>();
    for (const finding of this.findings) map.set(finding.id, finding);
    return map;
  });
  routineSuggestions = $state<RoutineSuggestion[]>([]);
  routines = $state<Routine[]>([]);
  history = $state<HistoryEntry[]>([]);

  /** Selected finding ids (deletion candidates). */
  selectedIds = $state<Set<string>>(new Set());
  cleaning = $state(false);

  /** Bumped on scope switch so transitions can reset. */
  treeVersion = $state(0);

  /** True outside the Tauri shell, when every command is served by the mock. */
  browserMode = $state(false);

  private toastSeq = 0;
  private started = false;
  private analysedScan = false;
  /**
   * Epoch id of the scan whose events may touch the current tree. Zero while a
   * new scan is starting, so late events from the previous scan are dropped.
   */
  private scanEpoch = 0;
  /** Resolvers waiting for a specific scan's done event during a scope switch. */
  private scanStopWaiters = new Map<number, () => void>();

  // ---- scope lookups ----

  get currentVolume(): VolumeInfo | null {
    return this.volumes.find((v) => v.id === this.scopeId) ?? null;
  }

  /** Root path of the active scan. */
  get scopePath(): string | null {
    return this.currentVolume?.mountPoint ?? null;
  }

  /** Display name of the active scope. */
  get scopeName(): string {
    return this.currentVolume?.name ?? '';
  }

  get currentNode(): Node | null {
    return this.currentNodeId
      ? (this.nodeRecords.get(this.currentNodeId) ?? null)
      : null;
  }

  get breadcrumbs(): Node[] {
    const chain: Node[] = [];
    let cur = this.currentNode;
    const guard = new Set<string>();
    while (cur && !guard.has(cur.id)) {
      guard.add(cur.id);
      chain.unshift(cur);
      cur = cur.parentId
        ? (this.nodeRecords.get(cur.parentId) ?? null)
        : null;
    }
    return chain;
  }

  /** Drill segments below the scan root (drives explorer crumbs). */
  get drillPath(): string[] {
    return this.breadcrumbs.slice(1).map((n) => n.name);
  }

  /**
   * Rebuild the reactive slice for the open folder from the plain records and
   * the live-fetched files. Runs after each batch, so per-folder work is
   * bounded by the open folder's entry count instead of tree size.
   */
  private rebuildCurrentEntries() {
    if (!this.currentNodeId) {
      this.listEntries = [];
      return;
    }
    const dirs: Node[] = [];
    for (const id of this.childIds.get(this.currentNodeId) ?? []) {
      const node = this.nodeRecords.get(id);
      if (node) dirs.push(node);
    }
    this.listEntries = [...dirs, ...this.currentFiles]
      .filter(
        (c) =>
          c.size > 0 ||
          c.isDir ||
          c.pending ||
          c.status === 'denied' ||
          c.status === 'awaiting'
      )
      .sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return b.size - a.size;
      })
      // Snapshot copies: published entries stay stable between rebuilds even
      // though the backing records keep mutating as the scan proceeds.
      .map((node) => ({ ...node }));
  }

  // ---- candidate getters ----

  get visible(): Finding[] {
    return this.findings.filter((f) => f.safety !== 'keep');
  }

  get smartItems(): Finding[] {
    return this.visible;
  }

  get candidates(): Finding[] {
    return this.visible.filter((f) => this.selectedIds.has(f.id));
  }

  get selectedBytes(): number {
    return this.candidates.reduce((s, f) => s + f.size, 0);
  }

  get safeBytes(): number {
    return this.sumBy('safe');
  }

  get reviewBytes(): number {
    return this.sumBy('review');
  }

  get keepBytes(): number {
    return this.sumBy('keep');
  }

  /** Everything actionable right now (safe + review) in the active scope. */
  get totalReclaimable(): number {
    return this.safeBytes + this.reviewBytes;
  }

  get safeReclaimable(): number {
    return this.safeBytes;
  }

  get pendingReviewCount(): number {
    return this.findings.filter((f) => f.safety === 'review').length;
  }

  get allTimeReclaimed(): number {
    return this.history.reduce((s, r) => s + r.bytes, 0);
  }

  get hasFindings(): boolean {
    return this.findings.some((f) => f.safety === 'safe' || f.safety === 'review');
  }

  private sumBy(risk: string): number {
    return this.findings
      .filter((f) => f.safety === risk)
      .reduce((s, f) => s + f.size, 0);
  }

  isSelected(id: string): boolean {
    return this.selectedIds.has(id);
  }

  /** Finding attached to a node, or undefined. Reactive when read in a template. */
  insightFor(id: string): Finding | undefined {
    return this.findingsById.get(id);
  }

  // ---- lifecycle ----

  async init() {
    if (this.started) return;
    this.started = true;

    this.browserMode = !isTauri();
    if (this.browserMode) this.toast(t('toast.browserMode'));

    await Promise.all([
      onScanBatch((batch) => this.handleBatch(batch)),
      onScanDone((event) => this.handleScanDone(event)),
      onScanRefreshed((event) => this.handleRefreshed(event)),
      onFsDeleted((event) => this.pruneIds(new Set([event.id]))),
      onMonitorEvent((event) => this.handleMonitorEvent(event))
    ]);

    // A reload (or a webview reattach) can happen while a scan is still
    // running on the backend; batches keep streaming either way, so reflect
    // the live scan instead of rendering the idle home screen. Adopting the
    // backend's epoch keeps those batches from being rejected as stale.
    const activeEpoch = await scanRunning();
    if (activeEpoch !== null) {
      this.scanning = true;
      this.scanEpoch = activeEpoch;
    }

    for (const warning of await takeStoreWarnings()) {
      this.toast(renderStoreWarning(warning));
    }

    this.volumes = await listVolumes();
    await this.loadHistory();
    await this.loadRoutines();

    // Pre-select the system disk without scanning; the user starts the first
    // scan explicitly from the home screen CTA.
    const preferred = this.volumes.find((v) => !v.isRemovable) ?? this.volumes[0];
    if (preferred) this.scopeId = preferred.id;

    this.refreshMonitorStatus();
  }

  // ---- scope selection ----

  async selectDisk(id: string) {
    await this.startScope(id);
  }

  private async startScope(id: string) {
    if (id === this.scopeId && this.rootId && !this.scanning) return;

    // Stop the current scan before switching: the backend rejects a second
    // concurrent start with "scan already running", and late batches from the
    // old scan could otherwise repopulate the reset tree (the first
    // parentId===null entry wins the root slot).
    if (this.scanning) await this.cancelForSwitch();

    this.scopeId = id;

    const root = this.scopePath;
    if (!root) return;

    // resetScanState clears scanEpoch to 0; no real scan carries epoch 0, so
    // every straggler event from the previous scan is rejected until the new
    // scan reports its id below.
    this.resetScanState();
    let epoch: number;
    try {
      epoch = await startScan(root, root);
    } catch (error) {
      this.scanning = false;
      this.toast(t('toast.scanStartFailed', [String(error)]));
      return;
    }
    this.scanEpoch = epoch;
    this.watchCurrentDir();
  }

  /**
   * Cancel the running scan as part of a scope switch and wait for its
   * `scan://done` event, so the backend's running flag has cleared before the
   * new start is attempted. A timeout bounds the wait in case the done event
   * is missed.
   */
  private async cancelForSwitch(): Promise<void> {
    const oldEpoch = this.scanEpoch;
    const stopped = new Promise<void>((resolve) => {
      this.scanStopWaiters.set(oldEpoch, resolve);
    });
    try {
      await cancelScan();
    } catch {
      // If the cancel itself fails, try the start anyway and surface its
      // error rather than blocking the switch.
    }
    await Promise.race([
      stopped,
      new Promise((resolve) => setTimeout(resolve, 3000)),
    ]);
    this.scanStopWaiters.delete(oldEpoch);
  }

  /** Cancel the running scan from the UI. */
  async cancelCurrentScan() {
    if (!this.scanning) return;
    await cancelScan();
    this.scanning = false;
    this.toast(t('toast.scanCancelled'));
  }

  private resetScanState() {
    this.nodeRecords = new Map();
    this.childIds = new Map();
    this.pathToId = new Map();
    this.findings = [];
    this.rootId = null;
    this.currentNodeId = null;
    this.currentFiles = [];
    this.listEntries = [];
    this.selectedIds = new Set();
    this.scanning = true;
    this.scannedFiles = 0;
    this.scannedDirs = 0;
    this.progress = {
      files: 0,
      dirs: 0,
      bytes: 0,
      totalBytes: 0,
      percent: 0,
      rateBytesPerSec: 0,
      awaiting: 0,
      trackedNodes: 0
    };
    this.permissionRequests = [];
    this.analysedScan = false;
    this.scanEpoch = 0;
    this.treeVersion += 1;
  }

  // ---- hub navigation ----

  go(view: 'home' | 'dashboard' | 'sub') {
    this.view = view;
  }

  goHome() {
    this.view = 'home';
  }

  goSub(tab: 'smart' | 'explorer' | 'history' | 'routines' = 'smart') {
    this.subTab = tab;
    this.view = 'sub';
  }

  setSubTab(tab: 'smart' | 'explorer' | 'history' | 'routines') {
    this.subTab = tab;
  }

  // ---- explorer drill ----

  drillInto(name: string) {
    const target = this.currentNode
      ? (this.childIds.get(this.currentNode.id) ?? []).find(
          (id) => this.nodeRecords.get(id)?.name === name
        )
      : undefined;
    const targetNode = target ? this.nodeRecords.get(target) : null;
    if (!targetNode || !targetNode.isDir) return;
    this.currentNodeId = targetNode.id;
    this.currentFiles = [];
    this.rebuildCurrentEntries();
    setScanFocus(targetNode.path);
    this.watchCurrentDir();
    this.refreshCurrentFiles();
  }

  jumpCrumb(index: number) {
    const target = this.breadcrumbs[index + 1];
    if (target) {
      this.currentNodeId = target.id;
      this.currentFiles = [];
      this.rebuildCurrentEntries();
      setScanFocus(target.path);
      this.watchCurrentDir();
      this.refreshCurrentFiles();
    }
  }

  goUp() {
    if (this.drillPath.length > 0) this.jumpCrumb(this.drillPath.length - 2);
  }

  // ---- scan event handling ----

  private handleMonitorEvent(event: unknown) {
    const data = event as {
      kind?: string;
      level?: string;
      message?: string;
      bytes?: number;
      paths?: string[];
      entryCount?: number;
      reason?: string;
    };
    switch (data.kind) {
      case 'notify':
        this.toast(t('toast.lowSpace'));
        break;
      case 'confirmationNeeded':
        this.toast(t('toast.confirmationNeeded'));
        break;
      case 'cleanupFinished': {
        const removed = new Set<string>();
        const removedPaths = new Set(data.paths ?? []);
        for (const path of removedPaths) {
          const id = this.pathToId.get(path);
          if (id) removed.add(id);
        }
        this.currentFiles = this.currentFiles.filter(
          (file) => !removedPaths.has(file.path)
        );
        if (removed.size > 0) {
          this.pruneIds(removed);
          this.findings = this.findings.filter((finding) => !removed.has(finding.id));
          this.selectedIds = new Set(
            [...this.selectedIds].filter((id) => !removed.has(id))
          );
        }
        this.loadHistory();
        this.loadRoutines();
        break;
      }
      case 'warning':
        if (data.message) this.toast(tr(data.message));
        break;
      default:
        break;
    }
  }

  private handleBatch(batch: BatchUpdate) {
    // Late batch from a cancelled/replaced scan: the new tree must not see it.
    if (batch.epoch !== this.scanEpoch) return;

    for (const incoming of batch.discovered) {
      // The pump only forwards directory entries; keep the guard anyway.
      if (!incoming.isDir) continue;
      const first = !this.rootId;
      const node: Node = { ...incoming, ext: extOf(incoming.name) };
      this.nodeRecords.set(node.id, node);
      this.pathToId.set(node.path, node.id);
      if (node.parentId) {
        const list = this.childIds.get(node.parentId);
        if (list) {
          if (!list.includes(node.id)) list.push(node.id);
        } else this.childIds.set(node.parentId, [node.id]);
      }
      if (first && node.parentId === null) {
        this.rootId = node.id;
        this.currentNodeId = node.id;
        // The scan root is the explorer's initial folder.
        this.rebuildCurrentEntries();
        this.refreshCurrentFiles();
      }
    }

    for (const sized of batch.sized) {
      const node = this.nodeRecords.get(sized.id);
      if (node) {
        // `null` size marks a status-only update; keep the last known size.
        if (sized.size !== null) node.size = sized.size;
        node.pending = sized.pending;
        node.status = sized.status;
      }
    }

    if (batch.permissions.length) {
      const known = new Set(this.permissionRequests.map((req) => req.id));
      this.permissionRequests = [
        ...this.permissionRequests,
        ...batch.permissions.filter((req) => !known.has(req.id))
      ];
    }

    if (batch.progress) {
      this.progress = batch.progress;
      this.scannedFiles = batch.progress.files;
      this.scannedDirs = batch.progress.dirs;
    }

    this.rebuildCurrentEntries();
  }

  /** Fetch the current folder's file entries live from the backend. */
  private async refreshCurrentFiles() {
    const node = this.currentNode;
    if (!node) return;
    let entries: Node[];
    try {
      entries = await listDirFiles(node.path);
    } catch {
      return;
    }
    // A navigation may have happened before the reply landed.
    if (this.currentNodeId !== node.id) return;
    this.currentFiles = entries.filter((entry) => !entry.isDir);
    // Marked state derives from findings (see insightFor), so the re-fetched
    // copies need no annotation and can never strip an existing mark.
    this.rebuildCurrentEntries();
  }

  private removePermissionRequest(id: string) {
    this.permissionRequests = this.permissionRequests.filter((req) => req.id !== id);
  }

  /**
   * Grant flow: the user picks the parked directory (or an ancestor such as
   * ~/Library, whose TCC grant covers every parked folder below it) in an
   * NSOpenPanel; selecting it itself grants the persistent TCC authorization.
   * The panel opens pointed at the requested path, then the engine re-walks.
   */
  async grantPermission(req: PermissionRequest) {
    let picked: string | null;
    try {
      picked = await pickDirectory(req.path);
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
      return;
    }
    // Panel cancelled: stay parked.
    if (!picked) return;

    if (!pathIsWithin(req.path, picked)) {
      this.toast(t('permission.mismatch'));
      return;
    }

    // One decision carrying the selected folder; the engine re-walks every
    // parked directory the grant covers. Cards under it disappear locally.
    try {
      await resolvePermission(req.id, true, picked);
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
      return;
    }
    const covered = this.permissionRequests.filter((other) =>
      pathIsWithin(other.path, picked));
    for (const other of covered) {
      this.removePermissionRequest(other.id);
    }
  }

  /** Explicitly mark the unresolved directory denied. */
  async skipPermission(req: PermissionRequest) {
    try {
      await resolvePermission(req.id, false);
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
      return;
    }
    this.removePermissionRequest(req.id);
  }

  /** Deny every grantable (TCC) directory in one action. Read-only system
   *  entries are not actionable and stay listed. */
  async skipAllPermissions() {
    const targets = this.permissionRequests.filter((req) => req.tcc);
    for (const req of targets) {
      try {
        await resolvePermission(req.id, false);
        this.removePermissionRequest(req.id);
      } catch (error) {
        this.toast(t('toast.settingsFailed', [String(error)]));
        return;
      }
    }
  }

  private async handleScanDone(event: ScanDoneEvent) {
    // This scan was cancelled to make room for a scope switch; release the
    // switch wait without touching the (about-to-be-reset) view state.
    const waiter = this.scanStopWaiters.get(event.epoch);
    if (waiter) {
      waiter();
      return;
    }
    // Done event of an older scan that is no longer displayed.
    if (event.epoch !== this.scanEpoch) return;

    this.scanning = false;
    if (event.cancelled) return;
    if (this.analysedScan) return;
    this.analysedScan = true;
    await this.runAnalysis();
  }

  /**
   * A post-completion authorization finished its refresh walk. Sized batches
   * already patched the records; re-render the open folder and re-run analysis
   * so smart-view conclusions reflect the newly visible bytes.
   */
  private async handleRefreshed(event: RefreshedEvent) {
    // Refresh from an older scan that is no longer displayed.
    if (event.epoch !== this.scanEpoch) return;
    this.rebuildCurrentEntries();
    await this.runAnalysis();
  }

  /** Ask the backend to analyse the finished scan and adopt the conclusions. */
  async runAnalysis() {
    try {
      const summary = await analyzeCurrent();
      this.findings = summary.findings;
      this.routineSuggestions = await routineSuggestions();

      const next = new Set(this.selectedIds);
      for (const finding of summary.findings) {
        if (finding.safety === 'safe') next.add(finding.id);
      }
      this.selectedIds = next;
    } catch (error) {
      this.toast(t('toast.analyzeFailed', [String(error)]));
    }
  }

  /** Open the settings sheet and load the persisted AI configuration. */
  async openSettings() {
    this.settingsOpen = true;
    try {
      this.aiConfig = await getAiConfig();
    } catch {
      this.aiConfig = null;
    }
  }

  /**
   * Persist the AI settings sheet. `token`: omitted keeps the stored secret,
   * '' clears it, a value replaces it. Returns whether the save succeeded so
   * the sheet can stay open for correction.
   */
  async saveAiSettings(
    config: Omit<AiConfig, 'hasToken' | 'batchSize'>,
    token?: string,
  ): Promise<boolean> {
    try {
      this.aiConfig = await saveAiConfig(config, token);
      this.toast(t('settings.aiSaved'));
      return true;
    } catch (error) {
      this.toast(t('toast.aiSaveFailed', [tr(String(error))]));
      return false;
    }
  }

  // ---- deletion ----

  async clean(automatic = false): Promise<number> {
    const targets = this.candidates;
    if (this.cleaning || targets.length === 0) return 0;
    this.cleaning = true;

    const paths = targets.map((f) => f.path);
    let results: DeleteResultItem[] = [];
    try {
      results = await cleanPaths(paths);
    } catch (error) {
      this.toast(t('toast.deleteFailed', [String(error)]));
      this.cleaning = false;
      return 0;
    }

    const removed = new Set<string>();
    let bytes = 0;
    for (const result of results) {
      if (result.ok) {
        const target = targets.find((f) => f.path === result.path);
        if (target) {
          removed.add(target.id);
          bytes += target.size;
        }
      } else {
        this.toast(t('toast.cannotDelete', [result.path, tr(result.error ?? '')]));
      }
    }

    this.pruneIds(removed);
    this.findings = this.findings.filter((f) => !removed.has(f.id));
    this.selectedIds = new Set();

    this.cleaning = false;
    await this.loadHistory();
    await this.loadRoutines();
    this.toast(
      automatic
        ? t('toast.autoCleanDone', [formatSize(bytes)])
        : t('toast.cleanDone', [formatSize(bytes)])
    );
    return bytes;
  }

  /**
   * Remove the given nodes and every descendant, in a single pass through the
   * parent links.
   */
  private pruneIds(roots: Set<string>) {
    if (roots.size === 0) return;
    const remove = new Set(roots);

    // Walk the plain child index to collect every descendant.
    const queue = [...roots];
    while (queue.length > 0) {
      const id = queue.pop()!;
      for (const child of this.childIds.get(id) ?? []) {
        if (remove.add(child)) queue.push(child);
      }
    }

    // Detach removed roots from parents that survive, before deleting records.
    const detach = new Map<string, string[]>();
    for (const id of remove) {
      const node = this.nodeRecords.get(id);
      if (node?.parentId && !remove.has(node.parentId)) {
        const list = detach.get(node.parentId) ?? [];
        list.push(id);
        detach.set(node.parentId, list);
      }
    }

    // All parent-chain lookups below must run while the node records still
    // exist; they are deleted later in this function.

    // Recompute surviving ancestor totals (R4.4): each removed component's
    // last known subtree size is subtracted up the surviving parent chain, so
    // the explorer stops showing bytes that are already gone.
    const sizeDeltas = prunedSizeDeltas(
      remove,
      (id) => this.nodeRecords.get(id)?.parentId ?? null,
      (id) => this.nodeRecords.get(id)?.size
    );
    for (const [id, delta] of sizeDeltas) {
      const node = this.nodeRecords.get(id);
      if (node) node.size = Math.max(0, node.size - delta);
    }

    // Resolve the post-prune view target BEFORE records are deleted: afterwards
    // every parent lookup returns undefined, which previously left the view
    // stranded on a deleted node with an empty directory listing.
    let nextCurrent: string | null = null;
    if (this.currentNodeId && remove.has(this.currentNodeId)) {
      nextCurrent = findSurvivingAncestor(this.currentNodeId, remove, (id) =>
        this.nodeRecords.get(id)?.parentId ?? null
      );
    }

    for (const id of remove) {
      const node = this.nodeRecords.get(id);
      if (node) this.pathToId.delete(node.path);
      this.nodeRecords.delete(id);
      this.childIds.delete(id);
    }
    for (const [parentId, removedChildren] of detach) {
      const list = this.childIds.get(parentId);
      if (list) {
        this.childIds.set(
          parentId,
          list.filter((id) => !removedChildren.includes(id))
        );
      }
    }

    this.currentFiles = this.currentFiles.filter((file) => !remove.has(file.id));

    if (this.currentNodeId && remove.has(this.currentNodeId)) {
      this.currentNodeId = nextCurrent;
      if (nextCurrent) {
        const node = this.nodeRecords.get(nextCurrent);
        if (node) setScanFocus(node.path);
      }
    }

    this.rebuildCurrentEntries();
  }

  // ---- selection ----

  /** Add a node to the deletion queue via the backend user-mark path. */
  async addManualCandidate(node: Node) {
    if (!node.deletable) {
      this.toast(t('toast.noDeletePermission', [node.name]));
      return;
    }
    const existing = this.findingsById.get(node.id);
    if (existing) {
      const next = new Set(this.selectedIds);
      next.add(existing.id);
      this.selectedIds = next;
      this.toast(t('toast.addedToQueue', [node.name]));
      return;
    }
    try {
      const finding = await markPath(node.path, node.size || undefined);
      // Append the finding first; rows derive their marked state from it, so
      // the keyed row flips regardless of snapshot/rebuild timing.
      this.findings = [...this.findings, finding];
      const next = new Set(this.selectedIds);
      next.add(finding.id);
      this.selectedIds = next;
      this.toast(t('toast.addedToQueue', [node.name]));
    } catch (error) {
      this.toast(t('toast.cannotAdd', [node.name, String(error)]));
    }
  }

  toggleSelected(id: string) {
    const next = new Set(this.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.selectedIds = next;
  }

  // ---- history & routines refresh ----

  async loadHistory() {
    try {
      this.history = await listHistory();
    } catch {
      // History is best-effort.
    }
  }

  async loadRoutines() {
    try {
      this.routines = await listRoutines();
    } catch {
      // Routines are best-effort.
    }
  }

  async acceptSuggestion(name: string, kind: string) {
    try {
      await acceptRoutineSuggestion(name, kind);
      await this.loadRoutines();
      this.routineSuggestions = this.routineSuggestions.filter(
        (s) => !(s.name === name && s.kind === kind)
      );
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  async dismissSuggestion(name: string, kind: string) {
    try {
      await dismissRoutineSuggestion(name, kind);
      this.routineSuggestions = this.routineSuggestions.filter(
        (s) => !(s.name === name && s.kind === kind)
      );
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  async deleteSavedRoutine(id: string) {
    try {
      await deleteRoutine(id);
      await this.loadRoutines();
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  async toggleSavedRoutineMode(id: string) {
    try {
      await toggleRoutineMode(id);
      await this.loadRoutines();
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  async runSavedRoutine(id: string) {
    try {
      await runRoutine(id);
      await this.loadHistory();
      await this.loadRoutines();
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  /** Add a Smart finding's cleanable family as a standing routine. */
  async addRoutineFromFinding(finding: Finding) {
    try {
      await saveRoutine(finding.name, finding.kind, finding.size);
      await this.loadRoutines();
      this.toast(t('toast.routineSaved', [finding.name]));
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  /** Add one Explorer item (a folder or file) as a path-based routine. */
  async addRoutineFromNode(node: Node) {
    try {
      await saveRoutine(node.name, '', node.size, node.path);
      await this.loadRoutines();
      this.toast(t('toast.routineSaved', [node.name]));
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  // ---- automatic mode & monitor ----

  async toggleAuto() {
    const nextMode = this.autoOn ? 'notify' : 'auto';
    try {
      await setAutoCleanMode(nextMode);
      this.autoOn = nextMode === 'auto';
      if (this.autoOn) {
        if (!this.monitorRunning) {
          await startMonitor();
          this.monitorRunning = true;
        }
        this.toast(t('toast.autoOn'));
      } else {
        this.toast(t('toast.autoOff'));
      }
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  async refreshMonitorStatus() {
    try {
      const status: MonitorStatusInfo = await monitorStatus();
      this.autoOn = status.autoMode === 'auto';
      this.monitorRunning = status.running;
      this.scheduledOn = status.scheduledCleanupEnabled;
      this.scheduledHour = status.scheduledHour;
    } catch {
      // Status is best-effort.
    }
  }

  async saveScheduled(enabled: boolean, hour: number) {
    try {
      await setScheduledCleanup(enabled, hour);
      this.scheduledOn = enabled;
      this.scheduledHour = hour;
    } catch (error) {
      this.toast(t('toast.settingsFailed', [String(error)]));
    }
  }

  // ---- focused directory watching ----

  private watchCurrentDir() {
    const path = this.currentNode?.path ?? this.scopePath;
    if (!path) return;
    watchFs(path, false).catch(() => undefined);
  }

  // ---- misc ----

  toast(message: string) {
    const id = ++this.toastSeq;
    this.toasts = [...this.toasts, { id, message }];
    setTimeout(() => {
      this.toasts = this.toasts.filter((t) => t.id !== id);
    }, 3200);
  }
}

function extOf(name: string): string {
  const lower = name.toLowerCase();
  const idx = lower.lastIndexOf('.');
  return idx >= 0 ? lower.slice(idx) : '';
}

function formatSize(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function stripTrailingSlash(path: string): string {
  return path.length > 1 ? path.replace(/\/+$/, '') : path;
}

/** Whether `path` is `ancestor` itself or a directory nested inside it. */
function pathIsWithin(path: string, ancestor: string): boolean {
  const a = stripTrailingSlash(ancestor);
  const p = stripTrailingSlash(path);
  return p === a || p.startsWith(`${a}/`);
}

export const store = new AppStore();
