import {
  listVolumes,
  onDiscovered,
  onProgress,
  onScanDone,
  onSized,
  setScanFocus,
  startScan
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
  /** Root directory id of the active scan. */
  rootId = $state<string | null>(null);
  /** Directory the user is currently viewing. */
  currentNodeId = $state<string | null>(null);

  scanning = $state(false);
  scannedFiles = $state(0);
  scannedDirs = $state(0);

  toasts = $state<Toast[]>([]);
  autoOn = $state(true);

  /** Routines (populated by the routines backend later). */
  routines = $state<Routine[]>([]);
  runningRoutineId = $state<string | null>(null);
  cleanedBytes = $state(0);

  /** Drawer visibility + cross-pane focus. */
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

  /** Root → current breadcrumb chain. */
  get breadcrumbs(): FileNode[] {
    const chain: FileNode[] = [];
    let cur = this.currentNode;
    while (cur) {
      chain.unshift(cur);
      cur = cur.parentId ? (this.nodes.get(cur.parentId) ?? null) : null;
    }
    return chain;
  }

  /** Children of the current directory: folders first, then by size. */
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

  /** Children carrying known sizes, used by the Treemap. */
  get tileEntries(): FileNode[] {
    return this.listEntries;
  }

  get totalSize(): number {
    return this.currentNode?.size ?? 0;
  }

  // ---- lifecycle -----------------------------------------------------------

  /** Initialise volumes and start the first scan exactly once. */
  async init() {
    if (this.started) return;
    this.started = true;

    const unlisten = await Promise.all([
      onDiscovered((n) => this.handleDiscovered(n)),
      onSized((e) => this.handleSized(e.id, e.size, e.pending)),
      onProgress((p) => {
        this.scannedFiles = p.files;
        this.scannedDirs = p.dirs;
      }),
      onScanDone((e) => {
        this.scanning = false;
        if (this.currentNode) {
          this.nodes.get(this.currentNode.id)!.pending = false;
        }
        if (this.autoOn && !e.cancelled) {
          // Automatic cleanup is wired in a later milestone.
        }
      })
    ]);

    this.volumes = await listVolumes();
    const preferred =
      this.volumes.find((v) => !v.isRemovable) ?? this.volumes[0];
    if (preferred) await this.selectVolume(preferred.id);

    return unlisten;
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

  // ---- volume selection ----------------------------------------------------

  async selectVolume(id: string) {
    if (id === this.currentVolumeId && this.rootId) return;
    this.currentVolumeId = id;
    const vol = this.volumes.find((v) => v.id === id);
    if (!vol) return;

    this.nodes = new Map();
    this.rootId = null;
    this.currentNodeId = null;
    this.scanning = true;
    this.scannedFiles = 0;
    this.scannedDirs = 0;
    await startScan(vol.mountPoint, vol.mountPoint);
  }

  // ---- navigation ----------------------------------------------------------

  drillInto(id: string) {
    this.currentNodeId = id;
    const n = this.nodes.get(id);
    if (n) setScanFocus(n.path);
  }

  jumpCrumb(index: number) {
    const chain = this.breadcrumbs;
    const target = chain[index];
    if (target) {
      this.currentNodeId = target.id;
      setScanFocus(target.path);
    }
  }

  goUp() {
    const cur = this.currentNode;
    if (cur?.parentId) {
      this.currentNodeId = cur.parentId;
      const p = this.nodes.get(cur.parentId);
      if (p) setScanFocus(p.path);
    }
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

export const store = new AppStore();
