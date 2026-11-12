/**
 * Recursive directory sizes, shared by every view and persisted across
 * launches. A folder total is expensive to compute (full subtree walk), so
 * the same answer is reused until it goes stale or the user asks to rescan.
 */
import { dirSizes } from './ipc';

interface CachedSize {
  size: number;
  at: number;
}

const STORAGE_KEY = 'sift.sizeCache.v1';
/** Totals older than this are recomputed; anything younger is reused. */
const TTL_MS = 30 * 60_000;
/** Hard cap on retained folders: the cache is core data, not a leak. */
const MAX_ENTRIES = 800;

let cache: Map<string, CachedSize> | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;
/** Concurrent callers asking for the same folder share one walk. */
const inflight = new Map<string, Promise<void>>();

function load(): Map<string, CachedSize> {
  if (cache) return cache;
  cache = new Map();
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Record<string, CachedSize>;
      for (const [path, entry] of Object.entries(parsed)) {
        if (entry && Number.isFinite(entry.size) && Number.isFinite(entry.at)) {
          cache.set(path, entry);
        }
      }
    }
  } catch {
    // A corrupt cache is just a cold cache.
  }
  return cache;
}

function scheduleFlush(): void {
  if (flushTimer) return;
  flushTimer = setTimeout(() => {
    flushTimer = undefined;
    try {
      const entries: Record<string, CachedSize> = {};
      for (const [path, entry] of load()) entries[path] = entry;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
    } catch {
      // Persistence is best-effort; the in-memory cache still works.
    }
  }, 500);
}

/** Fresh cached total for `path`, or null when missing/stale. */
export function peekSize(path: string): number | null {
  const hit = load().get(path);
  return hit && Date.now() - hit.at < TTL_MS ? hit.size : null;
}

/** Drop the oldest entries when the cache grows past its cap. */
function prune(): void {
  const map = load();
  if (map.size <= MAX_ENTRIES) return;
  const oldest = [...map.entries()].sort((a, b) => a[1].at - b[1].at);
  for (const [path] of oldest.slice(0, map.size - MAX_ENTRIES)) map.delete(path);
}

/** Totals for `paths`, recursively including every subdirectory. */
export async function sizesFor(
  paths: string[],
  force = false
): Promise<Map<string, number>> {
  const map = load();
  const now = Date.now();
  const out = new Map<string, number>();
  const misses: string[] = [];
  for (const path of paths) {
    const hit = map.get(path);
    if (!force && hit && now - hit.at < TTL_MS) out.set(path, hit.size);
    else misses.push(path);
  }
  const pending = misses.filter((path) => inflight.has(path));
  const fresh = misses.filter((path) => !inflight.has(path));
  if (fresh.length > 0) {
    const request = dirSizes(fresh).then((sizes) => {
      for (const size of sizes) {
        map.set(size.path, { size: size.size, at: now });
        out.set(size.path, size.size);
      }
    });
    for (const path of fresh) {
      inflight.set(
        path,
        request.finally(() => inflight.delete(path))
      );
    }
    await request;
    prune();
    scheduleFlush();
  }
  for (const path of pending) {
    // Someone else is already walking this folder: wait for that result.
    await inflight.get(path)?.catch(() => undefined);
    const hit = map.get(path);
    if (hit) out.set(path, hit.size);
  }
  return out;
}

/** Subtract bytes of a removed item from every cached ancestor folder. */
export function debitSize(path: string, bytes: number): void {
  if (bytes <= 0) return;
  const map = load();
  const now = Date.now();
  for (const [dir, entry] of map) {
    const prefix = dir === '/' ? '/' : dir.endsWith('/') ? dir : `${dir}/`;
    if (dir === path || path.startsWith(prefix)) {
      map.set(dir, { size: Math.max(0, entry.size - bytes), at: now });
    }
  }
  scheduleFlush();
}

/** Drop every cached total (manual rescan, or after a cleanup). */
export function clearSizeCache(): void {
  load().clear();
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}
