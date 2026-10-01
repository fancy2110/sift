import {
  listVolumes,
  homeDir,
  cancelScan,
  moveToTrash,
  onCalibrated,
  onCalibrationDone,
  onCalibrationStart,
  onDiscovered,
  onFsDeleted,
  onProgress,
  onScanDone,
  onSized,
  setScanFocus,
  startScan,
  watchFs,
  type DeleteResultItem
} from './ipc';
import { classify, INSIGHT_SPECS, type InsightKey } from './analyze';
import type { Node, Finding, Routine, VolumeInfo } from './types';

export interface Toast {
  id: number;
  message: string;
}

const ROUTINE_SEED: Routine[] = [
  { id: 'rt-downloads', title: '下载文件夹整理', cadence: '每周五 18:00', avgSize: 5.6 * 1024 ** 3, autoMode: 'auto' },
  { id: 'rt-cache', title: '开发缓存清理', cadence: '每周一 09:00', avgSize: 11.8 * 1024 ** 3, autoMode: 'approve' },
  { id: 'rt-space-guard', title: '空间守卫', cadence: '可用空间低于 15% 时', avgSize: 8.4 * 1024 ** 3, autoMode: 'auto' }
];

/**
 * Live, event-driven state. Nodes stream in from the backend scanner and
 * the current directory renders as soon as entries exist — never waiting
 * for the whole volume. AI findings are client-side aggregates whose
 * members are classified once per node (on discover / re-size).
 */
class AppStore {
  volumes = $state<VolumeInfo[]>([]);
  currentVolumeId = $state<string | null>(null);

  /** All discovered nodes keyed by stable id (per active scan). */
  nodes = $state<Map<string, Node>>(new Map());
  rootId = $state<string | null>(null);
  currentNodeId = $state<string | null>(null);

  scanning = $state(false);
  /** Main scan done; background calibration of giant dirs still running. */
  calibrating = $state(false);
  calibratedCount = $state(0);
  scannedFiles = $state(0);
  scannedDirs = $state(0);
  scannedBytes = $state(0);
  scanRateEntries = $state(0);
  scanRateBytes = $state(0);
  scanElapsed = $state(0);
  scanDenied = $state(0);
  scanCoverage = $state(0);

  toasts = $state<Toast[]>([]);
  autoOn = $state(false);

  routines = $state<Routine[]>(ROUTINE_SEED);
  runningRoutineId = $state<string | null>(null);
  cleanedBytes = $state(0);

  /** Selected finding ids (deletion candidates). */
  selectedIds = $state<Set<string>>(new Set());

  cleaning = $state(false);

  /// Bytes from the latest dry-run preview, awaiting user confirmation; 0 when
  /// no preview is pending.
  pendingConfirmBytes = $state(0);

  drawerOpen = $state(false);
  focusFinding = $state<string | null>(null);

  /** Member node ids per aggregated AI insight. */
  private members = $state<Map<InsightKey, Set<string>>>(new Map());
  /** Manual findings keyed by id; each holds one node id. */
  private manualMembers = $state<Map<string, string>>(new Map());
  private manualSeq = 0;

  /** Bumped on volume switch so transitions can reset. */
  treeVersion = $state(0);

  private toastSeq = 0;
  private started = false;

  // ---- lookups -------------------------------------------------------------

  get currentVolume(): VolumeInfo | null {
    return this.volumes.find((v) => v.id === this.currentVolumeId) ?? null;
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

  /** Drill segments below the volume root (drives Treemap key + crumbs). */
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

  // ---- AI findings ---------------------------------------------------------

  /** Live aggregated findings, sized from their members. */
  get findings(): Finding[] {
    const out: Finding[] = [];
    for (const [key, ids] of this.members) {
      if (ids.size === 0) continue;
      let size = 0;
      let path = '';
      for (const id of ids) {
        const n = this.nodes.get(id);
        if (!n) continue;
        size += n.size;
        if (!path) path = parentPath(n.path);
      }
      const spec = INSIGHT_SPECS[key];
      out.push({
        id: key,
        title: spec.title,
        reason: spec.reason(ids.size),
        size,
        path,
        risk: spec.risk,
        confidence: spec.confidence
      });
    }
    for (const [id, nodeId] of this.manualMembers) {
      const n = this.nodes.get(nodeId);
      if (!n) continue;
      out.push({
        id,
        title: n.name,
        reason: n.isDir
          ? '你手动加入的文件夹，将整体移入回收站，可恢复。'
          : '你手动加入的项目，将移入回收站，可恢复。',
        size: n.size,
        path: parentPath(n.path),
        risk: 'review',
        confidence: 1,
        manual: true
      });
    }
    const order: Record<string, number> = { safe: 0, review: 1, keep: 2 };
    return out.sort((a, b) => order[a.risk] - order[b.risk] || b.size - a.size);
  }

  /** Actionable (non-protected) findings shown in the candidate list. */
  get visible(): Finding[] {
    return this.findings.filter((f) => f.risk !== 'keep');
  }

  get candidates(): Finding[] {
    return this.visible.filter((f) => this.selectedIds.has(f.id));
  }

  get selectedBytes(): number {
    return this.candidates.reduce((s, f) => s + f.size, 0);
  }

  get safeBytes(): number {
    return sumBy(this.findings, 'safe');
  }

  get reviewBytes(): number {
    return sumBy(this.findings, 'review');
  }

  get keepBytes(): number {
    return sumBy(this.findings, 'keep');
  }

  get hasFindings(): boolean {
    return this.findings.some((f) => f.risk === 'safe' || f.risk === 'review');
  }

  isSelected(id: string): boolean {
    return this.selectedIds.has(id);
  }

  // ---- lifecycle -----------------------------------------------------------

  async init() {
    if (this.started) return;
    this.started = true;

    await Promise.all([
      onDiscovered((n) => this.handleDiscovered(n)),
      onSized((e) => this.handleSized(e.id, e.size, e.pending)),
      onCalibrated((e) => this.handleCalibrated(e.id, e.size, e.files)),
      onCalibrationStart(() => {
        this.calibrating = true;
        this.calibratedCount = 0;
      }),
      onCalibrationDone(() => {
        this.calibrating = false;
      }),
      onProgress((p) => {
        this.scannedFiles = p.files;
        this.scannedDirs = p.dirs;
        this.scannedBytes = p.bytes;
        this.scanRateEntries = p.entriesPerSec;
        this.scanRateBytes = p.bytesPerSec;
        this.scanElapsed = p.elapsedSecs;
        this.scanDenied = p.denied;
        this.scanCoverage = p.coverage;
      }),
      onScanDone(() => {
        this.scanning = false;
      }),
      onFsDeleted((e) => this.handleExternalDelete(e.id))
    ]);

    this.volumes = await listVolumes();
    const preferred = this.volumes.find((v) => !v.isRemovable) ?? this.volumes[0];
    if (preferred) await this.selectVolume(preferred.id);
  }

  // ---- event handling ------------------------------------------------------

  private handleDiscovered(incoming: Node) {
    const first = !this.rootId;
    const n: Node = { ...incoming, ext: extOf(incoming.name) };
    this.nodes.set(n.id, n);
    if (n.parentId) {
      const p = this.nodes.get(n.parentId);
      if (p) {
        p.children ??= [];
        if (!p.children.some((c) => c.id === n.id)) p.children.push(n);
      }
    }
    if (first && n.parentId === null) {
      this.rootId = n.id;
      this.currentNodeId = n.id;
    }
    this.reclassify(n);
    // Trigger reactivity for the assembled map / children.
    this.nodes = new Map(this.nodes);
  }

  private handleSized(id: string, size: number, pending: boolean) {
    const n = this.nodes.get(id);
    if (!n) return;
    n.size = size;
    n.pending = pending;
    this.reclassify(n);
  }

  /** A predicted giant subtree finished background measurement. */
  private handleCalibrated(id: string, size: number, files: number) {
    this.calibrating = true;
    this.calibratedCount += 1;
    const n = this.nodes.get(id);
    if (!n) return;
    n.size = size;
    n.pending = false;
    n.estimated = false;
    void files;
    this.reclassify(n);
    this.toast(`大型目录已校准：${n.name}`);
  }

  /** Assign a node to its heuristic insight; move it between member sets. */
  private reclassify(n: Node) {
    const key = n.deletable ? classify(n, Date.now()) : null;
    const current = n.insightId;
    // Manual findings own their node exclusively.
    if (current && this.manualMembers.has(current)) return;
    if (current === key) return;
    if (current) this.removeMember(current, n.id);
    if (key) this.addMember(key, n.id);
    n.insightId = key ?? undefined;
    n.risk = key ? INSIGHT_SPECS[key].risk : undefined;
    n.note = key ? INSIGHT_SPECS[key].title : undefined;
  }

  private addMember(key: string, id: string) {
    const set = this.members.get(key as InsightKey) ?? new Set<string>();
    const added = !set.has(id);
    set.add(id);
    this.members.set(key as InsightKey, set);
    if (added && key !== 'ai-protected') {
      // Safe findings are preselected once they first appear.
      if (INSIGHT_SPECS[key as InsightKey].risk === 'safe') {
        const next = new Set(this.selectedIds);
        next.add(key);
        this.selectedIds = next;
      }
    }
  }

  private removeMember(key: string, id: string) {
    const set = this.members.get(key as InsightKey);
    if (!set) return;
    set.delete(id);
  }

  /** A node vanished outside the app (or after our own trash call). */
  private handleExternalDelete(id: string) {
    if (!this.nodes.has(id)) return;
    this.pruneId(id);
  }

  private pruneId(id: string) {
    const remove = new Set<string>([id]);
    let grew = true;
    while (grew) {
      grew = false;
      for (const n of this.nodes.values()) {
        if (n.parentId && remove.has(n.parentId) && !remove.has(n.id)) {
          remove.add(n.id);
          grew = true;
        }
      }
    }

    // Detach from surviving parents, drop memberships / manual findings.
    const orphanedManual: string[] = [];
    for (const rid of remove) {
      const n = this.nodes.get(rid);
      if (!n) continue;
      if (n.parentId && !remove.has(n.parentId)) {
        const p = this.nodes.get(n.parentId);
        if (p?.children) p.children = p.children.filter((c) => c.id !== rid);
      }
      if (n.insightId) this.removeMember(n.insightId, rid);
      for (const [manualId, nodeId] of this.manualMembers) {
        if (nodeId === rid) orphanedManual.push(manualId);
      }
    }
    for (const m of orphanedManual) this.manualMembers.delete(m);

    const nextNodes = new Map<string, Node>();
    for (const [k, v] of this.nodes) if (!remove.has(k)) nextNodes.set(k, v);
    this.nodes = nextNodes;

    const nextSel = new Set(this.selectedIds);
    for (const r of remove) nextSel.delete(r);
    // Drop selections whose finding no longer has members.
    for (const fid of [...nextSel]) {
      if (!this.findings.some((f) => f.id === fid)) nextSel.delete(fid);
    }
    this.selectedIds = nextSel;

    if (this.currentNodeId && remove.has(this.currentNodeId)) {
      const start = this.nodes.get(id);
      let pid = start?.parentId ?? null;
      while (pid && !this.nodes.has(pid)) pid = this.nodes.get(pid)?.parentId ?? null;
      this.currentNodeId = pid;
      if (pid) {
        const n = this.nodes.get(pid);
        if (n) setScanFocus(n.path);
      }
    }
  }

  // ---- volume selection ----------------------------------------------------

  async selectVolume(id: string) {
    if (id === this.currentVolumeId && this.rootId && this.scanning) return;
    this.currentVolumeId = id;
    const vol = this.volumes.find((v) => v.id === id);
    if (!vol) return;

    this.nodes = new Map();
    this.members = new Map();
    this.manualMembers = new Map();
    this.rootId = null;
    this.currentNodeId = null;
    this.selectedIds = new Set();
    this.scanning = true;
    this.scannedFiles = 0;
    this.scannedDirs = 0;
    this.scannedBytes = 0;
    this.scanRateEntries = 0;
    this.scanRateBytes = 0;
    this.scanElapsed = 0;
    this.scanDenied = 0;
    this.scanCoverage = 0;
    this.drawerOpen = false;
    this.treeVersion += 1;
    // Built-in system volumes: deep-scan the user's home directory (system
    // files are not cleanable and walking them dominates the cold scan).
    // Removable drives are scanned in full.
    const scanRoot = vol.isRemovable ? vol.mountPoint : await this.userHomeRoot(vol.mountPoint);
    await startScan(scanRoot, scanRoot, vol.name);
    watchFs(vol.mountPoint).catch(() => undefined);
  }

  /** The user's home directory for a system volume, falling back to mount. */
  private async userHomeRoot(mountPoint: string): Promise<string> {
    try {
      const home = await homeDir();
      if (home) return home;
    } catch {
      // fall through
    }
    return mountPoint;
  }

  // ---- navigation ----------------------------------------------------------

  async cancelScan() {
    try {
      await cancelScan();
    } catch {
      // The scan may already have finished; the done event settles the state.
    }
  }

  drillIntoId(id: string) {
    const n = this.nodes.get(id);
    if (!n) return;
    this.currentNodeId = id;
    setScanFocus(n.path);
  }

  jumpCrumb(index: number) {
    const target = this.breadcrumbs[index];
    if (target) {
      this.currentNodeId = target.id;
      setScanFocus(target.path);
    }
  }

  // ---- manual candidates ---------------------------------------------------

  addManualCandidate(n: Node) {
    if (!n.deletable) {
      this.toast(`你没有删除「${n.name}」的权限`);
      return;
    }
    // Already part of an AI finding: just make sure it is selected.
    if (n.insightId && !this.manualMembers.has(n.insightId)) {
      const next = new Set(this.selectedIds);
      next.add(n.insightId);
      this.selectedIds = next;
      this.toast(`已将「${n.name}」加入删除队列`);
      return;
    }
    const id = `manual-${++this.manualSeq}-${Date.now().toString(36)}`;
    // Detach from any prior aggregated insight.
    if (n.insightId) this.removeMember(n.insightId, n.id);
    n.insightId = id;
    n.risk = 'review';
    n.note = '手动加入';
    this.manualMembers.set(id, n.id);
    const next = new Set(this.selectedIds);
    next.add(id);
    this.selectedIds = next;
    this.toast(`已将「${n.name}」加入删除队列`);
  }

  toggleSelected(id: string) {
    const next = new Set(this.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.selectedIds = next;
  }

  // ---- cleanup -------------------------------------------------------------

  async clean(automatic = false, confirmed = false): Promise<number> {
    const targets = this.candidates;
    if (this.cleaning || targets.length === 0) return 0;
    // Automatic cleanup (auto scan / routines) only runs when explicitly
    // enabled by the user; it defaults off and only reminds.
    if (automatic && !this.autoOn) return 0;

    const paths: string[] = [];
    for (const f of targets) {
      const ids = this.memberIdsOf(f.id);
      for (const nid of ids) {
        const n = this.nodes.get(nid);
        if (n) paths.push(n.path);
      }
    }

    // Default is dry-run: preview what would be reclaimed and require an
    // explicit second confirmation before anything really goes to trash.
    // Automatic cleanup with the toggle on is pre-authorized by the user.
    const execute = confirmed || automatic;
    this.cleaning = true;
    let results: DeleteResultItem[] = [];
    try {
      results = await moveToTrash(paths, execute);
    } catch (e) {
      this.toast(`删除失败：${String(e)}`);
      this.cleaning = false;
      return 0;
    }

    let bytes = 0;
    for (const r of results) {
      if (r.ok) {
        const nid = [...this.nodes.values()].find((n) => n.path === r.path)?.id;
        if (nid) bytes += this.nodes.get(nid)?.size ?? 0;
        // Dry-run must not remove nodes: the files still exist on disk.
        if (nid && !r.dryRun) this.pruneId(nid);
      } else {
        this.toast(`无法删除「${r.path}」：${r.error ?? ''}`);
      }
    }

    this.cleaning = false;
    if (!execute) {
      // Surface the preview and ask for confirmation; nothing was deleted.
      this.pendingConfirmBytes = bytes;
      this.toast(`预览：将可清理 ${fmt(bytes)}（未实际删除），请再次确认`);
      return 0;
    }
    this.pendingConfirmBytes = 0;
    this.cleanedBytes += bytes;
    this.selectedIds = new Set([...this.selectedIds].filter((id) => this.findings.some((f) => f.id === id)));
    this.drawerOpen = false;
    this.toast(
      automatic
        ? `自动整理完成，释放 ${fmt(bytes)}（文件在回收站，可恢复）`
        : `已释放 ${fmt(bytes)}（文件在回收站，可恢复）`
    );
    return bytes;
  }

  /** Confirm a dry-run preview and really move the files to trash. */
  async confirmClean() {
    if (this.pendingConfirmBytes === 0) return;
    await this.clean(false, true);
  }

  /** Dismiss the dry-run preview without deleting anything. */
  cancelCleanPreview() {
    this.pendingConfirmBytes = 0;
    this.toast('已取消清理，未删除任何文件');
  }

  private memberIdsOf(findingId: string): string[] {
    const ai = this.members.get(findingId as InsightKey);
    if (ai) return [...ai];
    const manual = this.manualMembers.get(findingId);
    return manual ? [manual] : [];
  }

  /** Select every safe finding and clean — used by auto / routines. */
  async runAuto() {
    if (!this.autoOn) return;
    const safe = this.findings.filter((f) => f.risk === 'safe').map((f) => f.id);
    this.selectedIds = new Set(safe);
    await this.clean(true);
  }

  async runRoutine(id: string) {
    const r = this.routines.find((x) => x.id === id);
    if (!r || this.runningRoutineId) return;
    this.runningRoutineId = id;
    this.toast(`「${r.title}」已启动`);
    const safe = this.findings.filter((f) => f.risk === 'safe').map((f) => f.id);
    this.selectedIds = new Set(safe);
    await this.clean(true);
    this.runningRoutineId = null;
  }

  deleteRoutine(id: string) {
    const r = this.routines.find((x) => x.id === id);
    if (!r) return;
    this.routines = this.routines.filter((x) => x.id !== id);
    this.toast(`已删除例行任务「${r.title}」`);
  }

  toggleAuto() {
    this.autoOn = !this.autoOn;
    if (this.autoOn) this.toast('自动整理已开启，安全项将按例行计划执行');
    else this.toast('自动整理已关闭，仅保留提醒');
  }

  toast(message: string) {
    const id = ++this.toastSeq;
    this.toasts = [...this.toasts, { id, message }];
    setTimeout(() => {
      this.toasts = this.toasts.filter((t) => t.id !== id);
    }, 3200);
  }
}

// ---- helpers ----------------------------------------------------------------

function extOf(name: string): string {
  const lower = name.toLowerCase();
  const idx = lower.lastIndexOf('.');
  return idx >= 0 ? lower.slice(idx) : '';
}

function parentPath(path: string): string {
  const idx = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return idx > 0 ? path.slice(0, idx) : path;
}

function sumBy(findings: Finding[], risk: string): number {
  return findings.filter((f) => f.risk === risk).reduce((s, f) => s + f.size, 0);
}

function fmt(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export const store = new AppStore();
