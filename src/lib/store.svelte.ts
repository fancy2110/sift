import {
  listVolumes,
  moveToTrash,
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
import type { FileNode, Routine, VolumeInfo } from './types';

export interface Toast {
  id: number;
  message: string;
}

/**
 * Live, event-driven filesystem state. Nodes stream in from the backend
 * scanner; the current directory renders as soon as its entries are
 * discovered — never waiting for the whole volume.
 */
class AppStore {
  volumes = $state<VolumeInfo[]>([]);
  currentVolumeId = $state<string | null>(null);

  /** All discovered nodes keyed by stable id (per active scan). */
  nodes = $state<Map<string, FileNode>>(new Map());
  rootId = $state<string | null>(null);
  currentNodeId = $state<string | null>(null);

  scanning = $state(false);
  scannedFiles = $state(0);
  scannedDirs = $state(0);

  toasts = $state<Toast[]>([]);
  autoOn = $state(true);

  routines = $state<Routine[]>([]);
  runningRoutineId = $state<string | null>(null);
  cleanedBytes = $state(0);

  /** Deletion candidate node ids. */
  selectedIds = $state<Set<string>>(new Set());
  cleaning = $state(false);

  drawerOpen = $state(false);
  focusNodeId = $state<string | null>(null);

  private toastSeq = 0;
  private started = false;

  get currentVolume(): VolumeInfo | null {
    return this.volumes.find((v) => v.id === this.currentVolumeId) ?? null;
  }

  get currentNode(): FileNode | null {
    return this.currentNodeId ? (this.nodes.get(this.currentNodeId) ?? null) : null;
  }

  get breadcrumbs(): FileNode[] {
    const chain: FileNode[] = [];
    let cur = this.currentNode;
    while (cur) {
      chain.unshift(cur);
      cur = cur.parentId ? (this.nodes.get(cur.parentId) ?? null) : null;
    }
    return chain;
  }

  get listEntries(): FileNode[] {
    const pid = this.currentNodeId;
    if (!pid) return [];
    const out: FileNode[] = [];
    for (const n of this.nodes.values()) {
      if (n.parentId === pid && n.size > 0) out.push(n);
    }
    return out.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return b.size - a.size;
    });
  }

  get tileEntries(): FileNode[] {
    return this.listEntries;
  }

  get totalSize(): number {
    return this.currentNode?.size ?? 0;
  }

  get candidateNodes(): FileNode[] {
    const out: FileNode[] = [];
    for (const id of this.selectedIds) {
      const n = this.nodes.get(id);
      if (n) out.push(n);
    }
    return out.sort((a, b) => b.size - a.size);
  }

  get selectedBytes(): number {
    return this.candidateNodes.reduce((s, n) => s + n.size, 0);
  }

  // ---- lifecycle -----------------------------------------------------------

  async init() {
    if (this.started) return;
    this.started = true;

    await Promise.all([
      onDiscovered((n) => this.handleDiscovered(n)),
      onSized((e) => this.handleSized(e.id, e.size, e.pending)),
      onProgress((p) => {
        this.scannedFiles = p.files;
        this.scannedDirs = p.dirs;
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

  private handleDiscovered(n: FileNode) {
    const first = !this.rootId;
    this.nodes.set(n.id, n);
    if (first && n.parentId === null) {
      this.rootId = n.id;
      this.currentNodeId = n.id;
    }
  }

  private handleSized(id: string, size: number, pending: boolean) {
    const n = this.nodes.get(id);
    if (n) {
      n.size = size;
      n.pending = pending;
    }
  }

  /** A node vanished outside the app (or after our own trash call). */
  private handleExternalDelete(id: string) {
    if (!this.nodes.has(id)) return;
    this.pruneId(id);
  }

  private pruneId(id: string) {
    // Collect the whole subtree for removal.
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
    const next = new Map<string, FileNode>();
    for (const [k, v] of this.nodes) {
      if (!remove.has(k)) next.set(k, v);
    }
    this.nodes = next;
    const sel = new Set(this.selectedIds);
    for (const r of remove) sel.delete(r);
    this.selectedIds = sel;
    if (remove.has(this.currentNodeId ?? '')) {
      const parent = this.nodes.get(id)?.parentId ?? null;
      this.currentNodeId = parent;
    }
  }

  // ---- volume selection ----------------------------------------------------

  async selectVolume(id: string) {
    if (id === this.currentVolumeId && this.rootId) return;
    this.currentVolumeId = id;
    const vol = this.volumes.find((v) => v.id === id);
    if (!vol) return;

    this.nodes = new Map();
    this.rootId = null;
    this.currentNodeId = null;
    this.selectedIds = new Set();
    this.scanning = true;
    this.scannedFiles = 0;
    this.scannedDirs = 0;
    await startScan(vol.mountPoint, vol.mountPoint);
    // Watch for external deletions; non-fatal where the backend lacks rights.
    watchFs(vol.mountPoint).catch(() => undefined);
  }

  // ---- navigation ----------------------------------------------------------

  drillInto(id: string) {
    this.currentNodeId = id;
    const n = this.nodes.get(id);
    if (n) setScanFocus(n.path);
  }

  jumpCrumb(index: number) {
    const target = this.breadcrumbs[index];
    if (target) {
      this.currentNodeId = target.id;
      setScanFocus(target.path);
    }
  }

  // ---- candidates ----------------------------------------------------------

  toggleSelected(id: string) {
    const n = this.nodes.get(id);
    if (!n || !n.deletable) return;
    const next = new Set(this.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.selectedIds = next;
  }

  async clean(): Promise<void> {
    const targets = this.candidateNodes;
    if (this.cleaning || targets.length === 0) return;
    this.cleaning = true;
    const paths = targets.map((n) => n.path);
    let results: DeleteResultItem[] = [];
    try {
      results = await moveToTrash(paths);
    } catch (e) {
      this.toast(`删除失败：${String(e)}`);
      this.cleaning = false;
      return;
    }

    const okPaths = results.filter((r) => r.ok).map((r) => r.path);
    const bytes = okPaths.reduce((s, p) => {
      const id = [...this.selectedIds].find((sid) => this.nodes.get(sid)?.path === p);
      return s + (id ? (this.nodes.get(id)?.size ?? 0) : 0);
    }, 0);

    for (const r of results) {
      if (!r.ok) this.toast(`无法删除「${r.path}」：${r.error ?? ''}`);
    }
    for (const p of okPaths) {
      const sid = targets.find((t) => t.path === p)?.id;
      if (sid) this.pruneId(sid);
    }

    this.cleanedBytes += bytes;
    this.selectedIds = new Set();
    this.cleaning = false;
    this.drawerOpen = false;
    this.toast(`已释放 ${gb(bytes)}（文件在回收站，可恢复）`);
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

function gb(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export const store = new AppStore();
