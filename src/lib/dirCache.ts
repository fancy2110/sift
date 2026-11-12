/**
 * Directory listings, shared by every visit so one read serves every view.
 *
 * A listing itself is cheap, but revisiting a folder after a tab switch used
 * to hit the filesystem again. Entries (and the recursive folder totals in
 * `sizeCache`) are cached together and persisted, so a scan made once is
 * reused everywhere until it goes stale or the user rescans.
 */
import { listDirEntries } from './ipc';
import type { DirEntry } from './ipc';

export interface DirListingSnapshot {
  path: string;
  entries: DirEntry[];
  at: number;
}

const STORAGE_KEY = 'sift.dirCache.v1';
const TTL_MS = 30 * 60_000;

let cache: Map<string, DirListingSnapshot> | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;

function load(): Map<string, DirListingSnapshot> {
  if (cache) return cache;
  cache = new Map();
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Record<string, DirListingSnapshot>;
      for (const [key, entry] of Object.entries(parsed)) {
        if (entry && Number.isFinite(entry.at) && Array.isArray(entry.entries)) {
          cache.set(key, entry);
        }
      }
    }
  } catch {
    // Corrupt cache: start cold.
  }
  return cache;
}

function scheduleFlush(): void {
  if (flushTimer) return;
  flushTimer = setTimeout(() => {
    flushTimer = undefined;
    try {
      const out: Record<string, DirListingSnapshot> = {};
      for (const [key, entry] of load()) out[key] = entry;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(out));
    } catch {
      // Best effort.
    }
  }, 500);
}

export async function listingFor(
  path: string,
  force = false
): Promise<DirListingSnapshot> {
  const map = load();
  const hit = map.get(path);
  if (!force && hit && Date.now() - hit.at < TTL_MS) return hit;
  const listing = await listDirEntries(path);
  const snapshot: DirListingSnapshot = {
    path: listing.path || path,
    entries: listing.entries.map((entry) =>
      entry.isDir ? { ...entry, size: 0 } : entry
    ),
    at: Date.now()
  };
  map.set(path, snapshot);
  map.set(snapshot.path, snapshot);
  scheduleFlush();
  return snapshot;
}

/** Remove a deleted path from cached listings and debit its ancestors. */
export function applyDeletion(path: string, bytes: number): void {
  const map = load();
  for (const [key, snapshot] of map) {
    const kept = snapshot.entries.filter(
      (entry) => entry.path !== path && !entry.path.startsWith(`${path}/`)
    );
    if (kept.length === snapshot.entries.length) continue;
    const adjusted = kept.map((entry) => {
      if (!entry.isDir) return entry;
      const prefix = entry.path.endsWith('/') ? entry.path : `${entry.path}/`;
      return path.startsWith(prefix)
        ? { ...entry, size: Math.max(0, entry.size - bytes) }
        : entry;
    });
    map.set(key, { ...snapshot, entries: adjusted });
  }
  scheduleFlush();
}

/** Replace the cached listing after sizes were filled in. */
export function updateListing(path: string, entries: DirEntry[]): void {
  const map = load();
  const snapshot: DirListingSnapshot = { path, entries, at: Date.now() };
  map.set(path, snapshot);
  scheduleFlush();
}

export function clearDirCache(): void {
  load().clear();
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}
