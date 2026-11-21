/**
 * Directory listings, served by the Rust scan index.
 *
 * The backend owns the listing cache (shared with every feature), so this
 * module only dedupes concurrent requests; the returned snapshot is the data
 * the index handed back, not a second cache.
 */
import { listDirEntries, clearDirListings } from './ipc';
import type { DirEntry } from './ipc';

export interface DirListingSnapshot {
  path: string;
  entries: DirEntry[];
  pending: boolean;
  at: number;
}

const inflight = new Map<string, Promise<DirListingSnapshot>>();

export async function listingFor(
  path: string,
  force = false
): Promise<DirListingSnapshot> {
  const key = `${force ? 'f' : 'c'}:${path}`;
  const pending = inflight.get(key);
  if (pending) return pending;
  const request = (async () => {
    // No timeout here: a slow response still carries useful data. The view
    // stops its spinner on its own without discarding the result.
    const listing = await listDirEntries(path, force);
    return {
      path: listing.path || path,
      entries: listing.entries,
      pending: Boolean(listing.pending),
      at: Date.now()
    };
  })();
  inflight.set(key, request);
  try {
    return await request;
  } finally {
    inflight.delete(key);
  }
}

/** Listings live in the backend now; kept as a no-op for existing callers. */
export function updateListing(_path: string, _entries: DirEntry[]): void {}

/** Deletions are applied to the shared index by the backend. */
export function applyDeletion(_path: string, _bytes: number): void {}

/** Kept for callers that used to inspect the local mirror; the backend owns it. */
export function peekListing(_path: string): null {
  return null;
}

/** Drop the backend's listing cache (manual rescan). */
export function clearDirCache(): void {
  void clearDirListings();
}
