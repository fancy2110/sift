/**
 * Pure tree-pruning helpers shared by the scan store.
 *
 * Kept free of Svelte/Tauri imports so the navigation logic can be reasoned
 * about (and executed) independently of the running app.
 */

/**
 * Return the nearest ancestor of `startId` that is not in `remove`, walking
 * parent links through `parentOf`, or `null` when no ancestor survives.
 *
 * `parentOf` MUST be evaluated while the node records still exist: once the
 * records have been deleted every lookup returns `undefined` and the walk
 * stops at the first step — which previously left the current view stranded
 * on a deleted node.
 */
export function findSurvivingAncestor(
  startId: string,
  remove: ReadonlySet<string>,
  parentOf: (id: string) => string | null | undefined
): string | null {
  let pid = parentOf(startId) ?? null;
  while (pid !== null && remove.has(pid)) {
    pid = parentOf(pid) ?? null;
  }
  return pid;
}
