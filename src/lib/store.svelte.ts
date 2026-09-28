import {
  analyzeCurrent,
  cancelScan,
  cleanPaths,
  deleteRoutine,
  dismissRoutineSuggestion,
  listHistory,
  listPlaces,
  listRoutines,
  listVolumes,
  markPath,
  monitorStatus,
  onFsDeleted,
  onMonitorEvent,
  onScanBatch,
  onScanDone,
  routineSuggestions,
  runRoutine,
  setAutoCleanMode,
  setScanFocus,
  startMonitor,
  startScan,
  takeStoreWarnings,
  toggleRoutineMode,
  watchFs,
  acceptRoutineSuggestion,
  type BatchUpdate,
  type DeleteResultItem
} from './ipc';
import type {
  Finding,
  HistoryEntry,
  MonitorStatus as MonitorStatusInfo,
  Node,
  Place,
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
  subTab = $state<'smart' | 'explorer' | 'history'>('smart');
  settingsOpen = $state(false);

  // ---- scan scopes ----
  volumes = $state<VolumeInfo[]>([]);
  places = $state<Place[]>([]);
  scopeKind = $state<'disk' | 'place'>('disk');
  scopeId = $state<string | null>(null);

  /** All discovered nodes keyed by stable id (per active scan). */
  nodes = $state<Map<string, Node>>(new Map());
  rootId = $state<string | null>(null);
  currentNodeId = $state<string | null>(null);

  scanning = $state(false);
  scannedFiles = $state(0);
  scannedDirs = $state(0);

  toasts = $state<Toast[]>([]);
  autoOn = $state(false);
  monitorRunning = $state(false);

  /** Analyzed candidates from the backend. */
  findings = $state<Finding[]>([]);
  routineSuggestions = $state<RoutineSuggestion[]>([]);
  routines = $state<Routine[]>([]);
  history = $state<HistoryEntry[]>([]);

  /** Selected finding ids (deletion candidates). */
  selectedIds = $state<Set<string>>(new Set());
  cleaning = $state(false);

  /** Bumped on scope switch so transitions can reset. */
  treeVersion = $state(0);

  private toastSeq = 0;
  private started = false;
  private analysedScan = false;

  // ---- scope lookups ----

  get currentVolume(): VolumeInfo | null {
    return this.scopeKind === 'disk'
      ? (this.volumes.find((v) => v.id === this.scopeId) ?? null)
      : null;
  }

  get currentPlace(): Place | null {
    return this.scopeKind === 'place'
      ? (this.places.find((p) => p.id === this.scopeId) ?? null)
      : null;
  }

  /** Root path of the active scan. */
  get scopePath(): string | null {
    return this.currentVolume?.mountPoint ?? this.currentPlace?.path ?? null;
  }

  /** Display name of the active scope. */
  get scopeName(): string {
    return this.currentVolume?.name ?? this.currentPlace?.labelKey ?? '';
  }

  get currentNode(): Node | null {
    return this.currentNodeId ? (this.nodes.get(this.currentNodeId) ?? null) : null;
  }

  get breadcrumbs(): Node[] {
    const chain: Node[] = [];
    let cur = this.currentNode;
    const guard = new Set<string>();
    while (cur && !guard.has(cur.id)) {
      guard.add(cur.id);
      chain.unshift(cur);
      cur = cur.parentId ? (this.nodes.get(cur.parentId) ?? null) : null;
    }
    return chain;
  }

  /** Drill segments below the scan root (drives explorer crumbs). */
  get drillPath(): string[] {
    return this.breadcrumbs.slice(1).map((n) => n.name);
  }

  /** Children of the current folder, folders first then size order. */
  get listEntries(): Node[] {
    return [...(this.currentNode?.children ?? [])]
      .filter((c) => c.size > 0 || c.pending)
      .sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return b.size - a.size;
      });
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

  // ---- lifecycle ----

  async init() {
    if (this.started) return;
    this.started = true;

    await Promise.all([
      onScanBatch((batch) => this.handleBatch(batch)),
      onScanDone((event) => this.handleScanDone(event.cancelled)),
      onFsDeleted((event) => this.pruneIds(new Set([event.id]))),
      onMonitorEvent((event) => this.handleMonitorEvent(event))
    ]);

    for (const warning of await takeStoreWarnings()) {
      this.toast(renderStoreWarning(warning));
    }

    this.volumes = await listVolumes();
    this.places = await listPlaces();
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
    this.scopeKind = 'disk';
    await this.startScope(id);
  }

  async selectPlace(id: string) {
    this.scopeKind = 'place';
    await this.startScope(id);
  }

  private async startScope(id: string) {
    if (id === this.scopeId && this.rootId && !this.scanning) return;
    this.scopeId = id;

    const root = this.scopePath;
    if (!root) return;

    this.resetScanState();
    try {
      await startScan(root, root);
    } catch (error) {
      this.scanning = false;
      this.toast(t('toast.scanStartFailed', [String(error)]));
      return;
    }
    this.watchCurrentDir();
  }

  /** Cancel the running scan from the UI. */
  async cancelCurrentScan() {
    if (!this.scanning) return;
    await cancelScan();
    this.scanning = false;
    this.toast(t('toast.scanCancelled'));
  }

  private resetScanState() {
    this.nodes = new Map();
    this.findings = [];
    this.rootId = null;
    this.currentNodeId = null;
    this.selectedIds = new Set();
    this.scanning = true;
    this.scannedFiles = 0;
    this.scannedDirs = 0;
    this.analysedScan = false;
    this.treeVersion += 1;
  }

  // ---- hub navigation ----

  go(view: 'home' | 'dashboard' | 'sub') {
    this.view = view;
  }

  goHome() {
    this.view = 'home';
  }

  goSub(tab: 'smart' | 'explorer' | 'history' = 'smart') {
    this.subTab = tab;
    this.view = 'sub';
  }

  setSubTab(tab: 'smart' | 'explorer' | 'history') {
    this.subTab = tab;
  }

  // ---- explorer drill ----

  drillInto(name: string) {
    const target = this.currentNode?.children?.find((c) => c.name === name);
    if (!target || !target.isDir) return;
    this.currentNodeId = target.id;
    setScanFocus(target.path);
    this.watchCurrentDir();
  }

  jumpCrumb(index: number) {
    const target = this.breadcrumbs[index + 1];
    if (target) {
      this.currentNodeId = target.id;
      setScanFocus(target.path);
      this.watchCurrentDir();
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
        for (const path of data.paths ?? []) {
          const id = [...this.nodes.values()].find((node) => node.path === path)?.id;
          if (id) removed.add(id);
        }
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
    for (const incoming of batch.discovered) {
      const first = !this.rootId;
      const node: Node = { ...incoming, ext: extOf(incoming.name) };
      this.nodes.set(node.id, node);
      if (node.parentId) {
        const parent = this.nodes.get(node.parentId);
        if (parent) {
          parent.children ??= [];
          if (!parent.children.some((c) => c.id === node.id)) parent.children.push(node);
        }
      }
      if (first && node.parentId === null) {
        this.rootId = node.id;
        this.currentNodeId = node.id;
      }
    }

    for (const sized of batch.sized) {
      const node = this.nodes.get(sized.id);
      if (node) {
        node.size = sized.size;
        node.pending = sized.pending;
      }
    }

    if (batch.progress) {
      this.scannedFiles = batch.progress.files;
      this.scannedDirs = batch.progress.dirs;
    }

    this.nodes = new Map(this.nodes);
    this.annotateNodes();
  }

  /** Reflect backend findings onto their nodes (insightId + risk). */
  private annotateNodes() {
    for (const finding of this.findings) {
      const node = this.nodes.get(finding.id);
      if (node && node.insightId !== finding.id) {
        node.insightId = finding.id;
        node.risk = finding.safety;
      }
    }
  }

  private async handleScanDone(cancelled: boolean) {
    this.scanning = false;
    if (cancelled) return;
    if (this.analysedScan) return;
    this.analysedScan = true;
    await this.runAnalysis();
  }

  /** Ask the backend to analyse the finished scan and adopt the conclusions. */
  async runAnalysis() {
    try {
      const summary = await analyzeCurrent();
      this.findings = summary.findings;
      this.routineSuggestions = await routineSuggestions();
      this.annotateNodes();

      const next = new Set(this.selectedIds);
      for (const finding of summary.findings) {
        if (finding.safety === 'safe') next.add(finding.id);
      }
      this.selectedIds = next;
    } catch (error) {
      this.toast(t('toast.analyzeFailed', [String(error)]));
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

    const byParent = new Map<string, string[]>();
    for (const node of this.nodes.values()) {
      if (node.parentId) {
        const list = byParent.get(node.parentId) ?? [];
        list.push(node.id);
        byParent.set(node.parentId, list);
      }
    }
    const queue = [...roots];
    while (queue.length > 0) {
      const id = queue.pop()!;
      for (const child of byParent.get(id) ?? []) {
        if (remove.add(child)) queue.push(child);
      }
    }

    for (const id of remove) {
      const node = this.nodes.get(id);
      if (node?.parentId && !remove.has(node.parentId)) {
        const parent = this.nodes.get(node.parentId);
        if (parent?.children) {
          parent.children = parent.children.filter((c) => c.id !== id);
        }
      }
    }

    const next = new Map<string, Node>();
    for (const [key, node] of this.nodes) {
      if (!remove.has(key)) next.set(key, node);
    }
    this.nodes = next;

    if (this.currentNodeId && remove.has(this.currentNodeId)) {
      let pid = this.nodes.get(this.currentNodeId)?.parentId ?? null;
      while (pid && !this.nodes.has(pid)) {
        pid = this.nodes.get(pid)?.parentId ?? null;
      }
      this.currentNodeId = pid;
      if (pid) {
        const node = this.nodes.get(pid);
        if (node) setScanFocus(node.path);
      }
    }
  }

  // ---- selection ----

  /** Add a node to the deletion queue via the backend user-mark path. */
  async addManualCandidate(node: Node) {
    if (!node.deletable) {
      this.toast(t('toast.noDeletePermission', [node.name]));
      return;
    }
    if (node.insightId) {
      const next = new Set(this.selectedIds);
      next.add(node.insightId);
      this.selectedIds = next;
      this.toast(t('toast.addedToQueue', [node.name]));
      return;
    }
    try {
      const finding = await markPath(node.path);
      this.findings = [...this.findings, finding];
      this.annotateNodes();
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
    } catch {
      // Status is best-effort.
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

export const store = new AppStore();
