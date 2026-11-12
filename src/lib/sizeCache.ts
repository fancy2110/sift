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

let cache: Map<string, CachedSize> | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;

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
  if (misses.length > 0) {
    const sizes = await dirSizes(misses);
    for (const size of sizes) {
      map.set(size.path, { size: size.size, at: now });
      out.set(size.path, size.size);
    }
    scheduleFlush();
  }
  return out;
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
