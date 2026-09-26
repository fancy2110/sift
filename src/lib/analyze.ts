import type { Node, Risk } from './types';

const MB = 1024 ** 2;
const GB = 1024 ** 3;
const DAY = 86_400_000;

const INSTALLER_EXT = ['.dmg', '.pkg', '.iso', '.zip', '.tgz', '.7z', '.rar', '.deb', '.exe', '.msi'];
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
  '.build',
  'Caches'
]);

export type InsightKey = 'ai-caches' | 'ai-installers' | 'ai-old-large' | 'ai-protected';

export interface InsightSpec {
  key: InsightKey;
  title: string;
  risk: Risk;
  confidence: number;
  reason: (count: number) => string;
}

export const INSIGHT_SPECS: Record<InsightKey, InsightSpec> = {
  'ai-caches': {
    key: 'ai-caches',
    title: '依赖与构建缓存',
    risk: 'safe',
    confidence: 0.92,
    reason: (c) => `${c} 个可重建的依赖 / 构建 / 缓存目录（如 node_modules、DerivedData），随时可重新生成。`
  },
  'ai-installers': {
    key: 'ai-installers',
    title: '安装包 / 压缩归档',
    risk: 'safe',
    confidence: 0.9,
    reason: (c) => `${c} 个安装镜像或压缩归档，对应软件通常已安装或可重新下载。`
  },
  'ai-old-large': {
    key: 'ai-old-large',
    title: '久未改动的大文件',
    risk: 'review',
    confidence: 0.7,
    reason: (c) => `${c} 个超过 200 MB 且 180 天未改动的文件，确认不再需要后可清理。`
  },
  'ai-protected': {
    key: 'ai-protected',
    title: '近期仍在使用的大文件',
    risk: 'keep',
    confidence: 0.85,
    reason: (c) => `${c} 个超过 5 GB 且近 120 天内使用过的文件，AI 已主动保留，避免误删。`
  }
};

function extOf(name: string): string {
  const lower = name.toLowerCase();
  const idx = lower.lastIndexOf('.');
  return idx >= 0 ? lower.slice(idx) : '';
}

/**
 * Classify a single streamed node heuristically. Membership of each
 * aggregated insight is maintained by the caller (the store); sizes are
 * always read live from the nodes, so late `sized` updates flow straight
 * into findings.
 */
export function classify(n: Node, now: number): InsightKey | null {
  if (!n.deletable) return null;
  if (n.isDir) {
    if (CACHE_DIRS.has(n.name)) return 'ai-caches';
    return null;
  }
  const ext = n.ext ?? extOf(n.name);
  if (INSTALLER_EXT.includes(ext) && n.size > 50 * MB) return 'ai-installers';
  if (n.size > 5 * GB && n.modifiedMs && now - n.modifiedMs < 120 * DAY) {
    return 'ai-protected';
  }
  if (n.size > 200 * MB && n.modifiedMs && now - n.modifiedMs > 180 * DAY) {
    return 'ai-old-large';
  }
  return null;
}
