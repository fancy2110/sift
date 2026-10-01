import type { FileNode, Insight, Risk, ScanLocation } from './types';
import { store } from './store.svelte';

const MB = 1024 ** 2;
const GB = 1024 ** 3;

const INSTALLER_EXT = ['.dmg', '.pkg', '.iso', '.zip', '.tar.gz', '.tgz', '.7z', '.rar'];
const CACHE_DIRS = new Set([
  'node_modules',
  '.gradle',
  'DerivedData',
  'build',
  '.cache',
  '.turbo',
  '.next',
  'target',
  'Pods',
  '.build'
]);

interface RawNode {
  name: string;
  size: number;
  children?: Map<string, RawNode>;
  isFile?: boolean;
  lastModified?: number;
  ext?: string;
}

function newNode(name: string): RawNode {
  return { name, size: 0 };
}

/** Build a size-aggregated tree from an <input webkitdirectory> FileList. */
export function buildTreeFromFiles(fileList: FileList): FileNode {
  const root: RawNode = newNode('root');
  let count = 0;
  for (const file of Array.from(fileList)) {
    if (++count > 80000) break;
    const rel = (file as File & { webkitRelativePath?: string }).webkitRelativePath ?? file.name;
    const parts = rel.split('/').filter(Boolean);
    let cur = root;
    parts.forEach((part, i) => {
      const isFile = i === parts.length - 1;
      if (isFile) {
        const leaf: RawNode = {
          name: part,
          size: file.size,
          isFile: true,
          lastModified: file.lastModified,
          ext: extOf(part)
        };
        cur.children ??= new Map();
        // Duplicate names: keep both under unique key, name preserved.
        cur.children.set(`${part}#${cur.children.size}`, leaf);
      } else {
        cur.children ??= new Map();
        let dir = cur.children.get(part);
        if (!dir) {
          dir = newNode(part);
          cur.children.set(part, dir);
        }
        cur = dir;
      }
    });
  }

  const aggregate = (n: RawNode): FileNode => {
    const out: FileNode = { name: n.name, size: n.size };
    if (n.isFile) {
      out.ext = n.ext;
      out.lastModified = n.lastModified;
    }
    if (n.children) {
      out.children = [];
      for (const child of n.children.values()) {
        const childOut = aggregate(child);
        out.size += childOut.size;
        out.children.push(childOut);
      }
      out.children.sort((a, b) => b.size - a.size);
    }
    return out;
  };
  return aggregate(root);
}

function extOf(name: string): string {
  const lower = name.toLowerCase();
  const tar = lower.match(/\.(tar\.[a-z0-9]+)$/);
  if (tar) return tar[1];
  const idx = lower.lastIndexOf('.');
  return idx >= 0 ? lower.slice(idx) : '';
}

/**
 * Heuristic local AI analysis of a picked folder.
 * Annotates the tree with insight ids and returns generated insights.
 */
export function analyzeFolderTree(tree: FileNode, locSeq: number): { tree: FileNode; insights: Insight[] } {
  const prefix = `cust${locSeq}`;
  const installerId = `${prefix}-installers`;
  const cacheId = `${prefix}-caches`;
  const oldId = `${prefix}-old-large`;
  const dupId = `${prefix}-duplicates`;

  const bytes = { installers: 0, caches: 0, old: 0 };
  const counts = { installers: 0, caches: 0, old: 0 };
  const seen = new Map<string, FileNode[]>();
  const now = Date.now();

  const walk = (n: FileNode) => {
    if (n.children) {
      if (CACHE_DIRS.has(n.name)) {
        n.insightId = cacheId;
        n.risk = 'safe';
        n.note = '可重建的依赖 / 构建产物';
        bytes.caches += n.size;
        counts.caches += 1;
        return;
      }
      for (const c of n.children) walk(c);
      return;
    }
    // Leaf file
    if (n.ext && INSTALLER_EXT.includes(n.ext) && n.size > 50 * MB) {
      n.insightId = installerId;
      n.risk = 'safe';
      bytes.installers += n.size;
      counts.installers += 1;
      return;
    }
    if (n.size > 200 * MB) {
      const ageDays = n.lastModified ? (now - n.lastModified) / 86_400_000 : 0;
      if (ageDays > 180) {
        n.insightId = oldId;
        n.risk = 'review';
        bytes.old += n.size;
        counts.old += 1;
      }
      const key = `${n.name}|${n.size}`;
      const list = seen.get(key) ?? [];
      list.push(n);
      seen.set(key, list);
    }
  };
  tree.children?.forEach(walk);

  // Duplicate names with identical sizes → mark every copy.
  let dupBytes = 0;
  let dupGroups = 0;
  for (const nodes of seen.values()) {
    if (nodes.length > 1) {
      dupGroups += 1;
      for (const n of nodes) {
        n.insightId = dupId;
        n.risk = 'review';
        n.note = '同名且同大小，可能重复';
        dupBytes += n.size;
      }
    }
  }

  const locId = `loc-custom-${prefix}`;
  const insights: Insight[] = [];
  if (bytes.installers > 0) {
    insights.push({
      id: installerId,
      title: '安装包 / 压缩归档',
      reason: `${counts.installers} 个安装镜像或归档文件，对应软件通常已安装或可重新下载。`,
      size: bytes.installers,
      path: tree.name,
      risk: 'safe',
      confidence: 0.9,
      locId
    });
  }
  if (bytes.caches > 0) {
    insights.push({
      id: cacheId,
      title: '依赖与构建缓存',
      reason: `${counts.caches} 个可重建的依赖 / 构建目录（如 node_modules、build），随时可重新生成。`,
      size: bytes.caches,
      path: tree.name,
      risk: 'safe',
      confidence: 0.92,
      locId
    });
  }
  if (dupBytes > 0) {
    insights.push({
      id: dupId,
      title: '可能重复的文件',
      reason: `${dupGroups} 组同名且大小完全一致的文件，建议确认后每组保留一份。`,
      size: dupBytes,
      path: tree.name,
      risk: 'review',
      confidence: 0.8,
      locId
    });
  }
  if (bytes.old > 0) {
    insights.push({
      id: oldId,
      title: '久未改动的大文件',
      reason: `${counts.old} 个文件超过 200 MB 且 180 天未改动，确认不再需要后可清理。`,
      size: bytes.old,
      path: tree.name,
      risk: 'review',
      confidence: 0.7,
      locId
    });
  }

  return { tree, insights };
}

let customSeq = 0;

/** Read a picked folder via the native folder input and register it. */
export async function pickAndRegisterFolder(): Promise<{ loc: ScanLocation } | null> {
  const input = document.createElement('input');
  input.type = 'file';
  (input as HTMLInputElement & { webkitdirectory: boolean }).webkitdirectory = true;

  const chosen = new Promise<FileList | null>((resolve) => {
    input.onchange = () => resolve(input.files && input.files.length > 0 ? input.files : null);
  });
  input.click();
  const files = await chosen;
  if (!files || files.length === 0) return null;

  customSeq += 1;
  const seq = customSeq;
  const firstPath = (files[0] as File & { webkitRelativePath?: string }).webkitRelativePath ?? files[0].name;
  const folderName = firstPath.split('/')[0] || `文件夹 ${seq}`;

  const rawTree = buildTreeFromFiles(files);
  const renamed: FileNode = { ...rawTree, name: folderName };
  const { tree, insights } = analyzeFolderTree(renamed, seq);

  const id = `loc-custom-cust${seq}`;
  const loc: ScanLocation = {
    id,
    name: folderName,
    path: `已选文件夹 · ${folderName}`,
    icon: 'folder',
    group: 'places',
    diskId: 'loc-disk',
    custom: true
  };

  store.addCustomLocation(loc, tree, insights);
  return { loc };
}

export function demoFmt(bytes: number): string {
  return bytes >= GB ? `${(bytes / GB).toFixed(1)} GB` : `${Math.round(bytes / MB)} MB`;
}
