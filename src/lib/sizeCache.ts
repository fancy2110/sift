/**
 * Recursive directory sizes, shared by every view and persisted across
 * launches. A folder total is expensive to compute (full subtree walk), so
 * the same answer is reused until it goes stale or the user asks to rescan.
 */
import { dirSizesTree, clearSizeTree, dirSizePeek } from './ipc';

interface CachedSize {
  size: number;
  at: number;
  /** true when the value came from a full walk of this path; derived sums
   * are false so an exact walk is never double-counted. */
  exact?: boolean;
}

/** Totals older than this are recomputed; anything younger is reused. */
const TTL_MS = 30 * 60_000;
/** Hard cap on retained folders: the cache is core data, not a leak. */
const MAX_ENTRIES = 800;

let cache: Map<string, CachedSize> | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;
/** Concurrent callers asking for the same folder share one walk. */
const inflight = new Map<string, Promise<void>>();

function load(): Map<string, CachedSize> {
  if (!cache) cache = new Map();
  return cache;
}

function scheduleFlush(): void {
  // Persistence belongs to the Rust index; this map only mirrors the values
  // needed for synchronous rendering in this session.
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
  // Hydrate from the backend's authoritative tree first; only walk what it
  // does not know (or when the user forces a rescan).
  if (misses.length > 0) {
    const peeked = await dirSizePeek(misses);
    for (const size of peeked) {
      if (size.size > 0) {
        map.set(size.path, { size: size.size, at: now, exact: false });
        out.set(size.path, size.size);
      }
    }
  }
  const pending = misses.filter((path) => inflight.has(path) && !out.has(path));
  const fresh = misses.filter((path) => !inflight.has(path) && !out.has(path));
  if (fresh.length > 0) {
    const request = dirSizesTree(fresh).then((sizes: Awaited<ReturnType<typeof dirSizesTree>>) => {
      for (const size of sizes) {
        map.set(size.path, { size: size.size, at: now, exact: true });
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

/** Record a size that is already known (e.g. a volume's used bytes) without
 * walking the subtree. */
export function seedSize(path: string, size: number): void {
  load().set(path, { size, at: Date.now(), exact: true });
  scheduleFlush();
}

/** True when `dir` is `path` or one of its ancestors. */
function isAncestor(dir: string, path: string): boolean {
  if (dir === path) return true;
  const prefix = dir === '/' ? '/' : dir.endsWith('/') ? dir : `${dir}/`;
  return path.startsWith(prefix);
}

/** Push a size change up the cached ancestors (exact walks stay untouched). */
function propagate(map: Map<string, CachedSize>, path: string, delta: number): void {
  if (delta === 0) return;
  const now = Date.now();
  for (const [dir, entry] of map) {
    if (dir === path || !isAncestor(dir, path)) continue;
    if (entry.exact) continue;
    map.set(dir, { size: Math.max(0, entry.size + delta), at: now, exact: false });
  }
}

/**
 * Record a folder's total assembled from its children (the listing sum) and
 * recursively update cached ancestors, so a parent's number always reflects
 * folders that finished measuring after it did.
 */
export function setDerivedSize(path: string, size: number, force = false): void {
  const map = load();
  const previous = map.get(path);
  if (previous?.exact && !force) return;
  const before = previous?.size ?? 0;
  map.set(path, { size, at: Date.now(), exact: false });
  propagate(map, path, size - before);
  scheduleFlush();
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
  void clearSizeTree();
}
