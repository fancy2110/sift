import {
  insights as seedInsights,
  routines as seedRoutines,
  fileTrees,
  scanLocations,
  volumes
} from './data';
import type { FileNode, Insight, Routine, ScanLocation, Volume } from './types';

export interface Toast {
  id: number;
  message: string;
}

/** Insights considered "inside" each scope. */
const SCOPE: Record<string, string[]> = {
  'loc-disk': ['loc-user', 'loc-downloads', 'loc-desktop', 'loc-movies', 'loc-trash'],
  'loc-user': ['loc-user', 'loc-downloads', 'loc-desktop', 'loc-movies', 'loc-trash'],
  'loc-downloads': ['loc-downloads'],
  'loc-desktop': ['loc-desktop'],
  'loc-movies': ['loc-movies'],
  'loc-trash': ['loc-trash'],
  'loc-backup-disk': ['loc-backup-disk']
};

export function findSubtree(root: FileNode, path: string[] | undefined): FileNode {
  if (!path || path.length === 0) return root;
  let cur = root;
  for (const seg of path) {
    const next = cur.children?.find((c) => c.name === seg);
    if (!next) break;
    cur = next;
  }
  return cur;
}

function cloneDisks(): Record<string, FileNode> {
  const out: Record<string, FileNode> = {};
  for (const [id, tree] of Object.entries(fileTrees)) out[id] = structuredClone(tree);
  return out;
}

class AppStore {
  insights = $state<Insight[]>(seedInsights);
  routines = $state<Routine[]>(seedRoutines);
  locations = $state<ScanLocation[]>(scanLocations);
  currentLocId = $state('loc-disk');
  selectedIds = $state<Set<string>>(
    new Set(['ins-derived-data', 'ins-package-caches', 'ins-download-dmg', 'ins-trash'])
  );
  resolvedIds = $state<Set<string>>(new Set());
  toasts = $state<Toast[]>([]);
  autoOn = $state(true);
  cleaning = $state(false);
  runningRoutineId = $state<string | null>(null);
  cleanedBytes = $state(0);

  /** Breadcrumb drill path inside the current scope. */
  drillPath = $state<string[]>([]);
  /** +1 drilled in, -1 jumped up — drives map transition direction. */
  navDir = $state(1);
  /** Drawer visibility + cross-surface insight highlight. */
  drawerOpen = $state(false);
  focusInsight = $state<string | null>(null);

  /** Live per-disk trees (resolved findings pruned), keyed by volume id. */
  diskTrees = $state<Record<string, FileNode>>(cloneDisks());
  /** Roots generated from user-picked folders, keyed by location id. */
  customTrees: Record<string, FileNode> = {};

  /** Bumped after cleanup / location switch so views can reset transitions. */
  treeVersion = $state(0);

  private toastSeq = 0;

  get currentLocation(): ScanLocation {
    return this.locations.find((l) => l.id === this.currentLocId) ?? this.locations[0];
  }

  get diskLocations(): ScanLocation[] {
    return this.locations.filter((l) => l.group === 'disk');
  }

  get placeLocations(): ScanLocation[] {
    return this.locations.filter((l) => l.group === 'places');
  }

  /** Live tree for the selected scope. */
  get treeRoot(): FileNode {
    if (this.currentLocation.custom) {
      return this.customTrees[this.currentLocId] ?? this.diskTrees[this.currentLocation.diskId];
    }
    const diskTree = this.diskTrees[this.currentLocation.diskId];
    return findSubtree(diskTree, this.currentLocation.subtreePath);
  }

  /** Folder currently shown by the list / map. */
  get currentNode(): FileNode {
    return findSubtree(this.treeRoot, this.drillPath);
  }

  /** Owning volume with live usage (used shrinks as findings resolve). */
  get currentVolume(): Volume {
    const seed = volumes.find((v) => v.id === this.currentLocation.diskId) ?? volumes[0];
    const liveTree = this.diskTrees[seed.id];
    return liveTree ? { ...seed, used: liveTree.size } : seed;
  }

  /** Children of the current folder, folders first within size order. */
  get listEntries(): FileNode[] {
    return [...(this.currentNode.children ?? [])]
      .filter((c) => c.size > 0)
      .sort((a, b) => b.size - a.size);
  }

  /** Insights belonging to the current scope. */
  get scopedInsights(): Insight[] {
    if (this.currentLocation.custom) {
      return this.insights.filter((i) => i.locId === this.currentLocId);
    }
    const allowed = SCOPE[this.currentLocId] ?? [this.currentLocId];
    return this.insights.filter((i) => allowed.includes(i.locId));
  }

  get visible(): Insight[] {
    return this.scopedInsights.filter((i) => !this.resolvedIds.has(i.id));
  }

  get candidates(): Insight[] {
    return this.visible.filter((i) => i.risk !== 'keep' && this.selectedIds.has(i.id));
  }

  get selectedBytes(): number {
    return this.candidates.reduce((sum, i) => sum + i.size, 0);
  }

  /** Short "used / capacity" label for a disk entry. */
  diskUsage(diskId: string): string {
    const tree = this.diskTrees[diskId];
    const seed = volumes.find((v) => v.id === diskId);
    if (!tree || !seed) return '';
    const pct = Math.round((tree.size / seed.capacity) * 100);
    return `${pct}% 已使用 · ${seed.external ? '外置磁盘' : '内置磁盘'}`;
  }

  isSelected(id: string): boolean {
    return this.selectedIds.has(id);
  }

  // ---- fast navigation ----------------------------------------------------

  drillInto(name: string) {
    this.navDir = 1;
    this.drillPath = [...this.drillPath, name];
  }

  /** Jump to crumb index; -1 returns to the scope root. */
  jumpCrumb(index: number) {
    this.navDir = -1;
    this.drillPath = index < 0 ? [] : this.drillPath.slice(0, index + 1);
  }

  goUp() {
    if (this.drillPath.length === 0) return;
    this.jumpCrumb(this.drillPath.length - 2);
  }

  setLocation(id: string) {
    if (id === this.currentLocId) return;
    this.currentLocId = id;
    this.navDir = 1;
    this.drillPath = [];
    const loc = this.locations.find((l) => l.id === id);
    // Default candidates: safe items in the new scope.
    const scope = new Set(
      loc?.custom ? [id] : (SCOPE[id] ?? [id])
    );
    this.selectedIds = new Set(
      this.insights
        .filter(
          (i) =>
            i.risk === 'safe' &&
            (loc?.custom ? i.locId === id : scope.has(i.locId))
        )
        .map((i) => i.id)
    );
    this.treeVersion += 1;
  }

  // ---- candidate queue ----------------------------------------------------

  toggleSelected(id: string) {
    const next = new Set(this.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.selectedIds = next;
  }

  private manualSeq = 0;

  /**
   * Right-click → "添加到删除列表" on a tile that has no AI insight.
   * Registers a manual insight bound to the live tree node so it flows
   * through the same candidate / cleanup pipeline as AI findings.
   */
  addManualCandidate(node: FileNode) {
    if (node.deletable === false) {
      this.toast(`你没有删除「${node.name}」的权限`);
      return;
    }
    // Already an actionable item: just make sure it is selected.
    if (node.insightId) {
      if (!this.selectedIds.has(node.insightId)) this.toggleSelected(node.insightId);
      this.toast(`已将「${node.name}」加入删除列表`);
      return;
    }
    const id = `manual-${++this.manualSeq}-${Date.now().toString(36)}`;
    const fullPath = [this.currentLocation.name, ...this.drillPath, node.name].join('/');
    const insight: Insight = {
      id,
      title: node.name,
      reason: node.children?.length
        ? `你手动加入的文件夹（含 ${node.children.length} 个项目），将整体移入废纸篓，可恢复。`
        : '你手动加入的项目，将移入废纸篓，可恢复。',
      size: node.size,
      path: fullPath,
      risk: 'review',
      confidence: 1,
      manual: true,
      locId: this.currentLocId
    };
    node.insightId = id;
    this.insights = [...this.insights, insight];
    this.toggleSelected(id);
    this.toast(`已将「${node.name}」加入删除列表`);
  }

  /** Register a user-picked folder: its tree + heuristic AI findings. */
  addCustomLocation(loc: ScanLocation, tree: FileNode, generated: Insight[]) {
    this.locations = [...this.locations, loc];
    this.customTrees[loc.id] = tree;
    this.insights = [...this.insights, ...generated];
    this.setLocation(loc.id);
  }

  // ---- cleanup execution --------------------------------------------------

  async clean(automatic = false) {
    const targets = this.candidates;
    if (this.cleaning || targets.length === 0) return;
    this.cleaning = true;
    // Simulated cleanup — real build wires this to the privileged helper / Trash API.
    await new Promise((r) => setTimeout(r, 1200));
    const bytes = targets.reduce((sum, i) => sum + i.size, 0);
    const resolved = new Set(this.resolvedIds);
    for (const t of targets) resolved.add(t.id);
    this.resolvedIds = resolved;
    this.selectedIds = new Set();
    this.cleanedBytes += bytes;
    this.cleaning = false;
    // Re-prune every disk tree from its seed (resolved ids live on one disk).
    const next: Record<string, FileNode> = {};
    for (const [diskId, seed] of Object.entries(fileTrees)) {
      next[diskId] = this.pruneTree(structuredClone(seed), resolved);
    }
    this.diskTrees = next;
    this.navDir = -1;
    this.drillPath = [];
    this.treeVersion += 1;
    this.toast(
      automatic
        ? `自动整理完成，释放 ${fmt(bytes)}（文件在废纸篓，可恢复）`
        : `已释放 ${fmt(bytes)}（文件在废纸篓，可恢复）`
    );
  }

  /** Fires the learned routine immediately: clean all safe findings. */
  async runAuto() {
    if (!this.autoOn) return;
    this.selectedIds = new Set(
      this.visible.filter((i) => i.risk === 'safe').map((i) => i.id)
    );
    await this.clean(true);
  }

  // ---- routines -----------------------------------------------------------

  /** Start a learned routine immediately. */
  async runRoutine(id: string) {
    const r = this.routines.find((x) => x.id === id);
    if (!r || this.cleaning) return;
    this.runningRoutineId = id;
    this.toast(`「${r.title}」已启动`);
    // Demo: run the routine against the current scope's safe findings.
    this.selectedIds = new Set(
      this.visible.filter((i) => i.risk === 'safe').map((i) => i.id)
    );
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

  /** Remove resolved-insight nodes and subtract their size from ancestors. */
  private pruneTree(source: FileNode, resolved: Set<string>): FileNode {
    const walk = (n: FileNode): FileNode | null => {
      if (n.insightId && resolved.has(n.insightId)) return null;
      if (n.children) {
        const kept: FileNode[] = [];
        let removed = 0;
        for (const c of n.children) {
          const w = walk(c);
          if (w) kept.push(w);
          else removed += c.size;
        }
        n.children = kept;
        n.size -= removed;
      }
      return n;
    };
    walk(source);
    return source;
  }

  toast(message: string) {
    const id = ++this.toastSeq;
    this.toasts = [...this.toasts, { id, message }];
    setTimeout(() => {
      this.toasts = this.toasts.filter((t) => t.id !== id);
    }, 3200);
  }
}

function fmt(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export const store = new AppStore();
