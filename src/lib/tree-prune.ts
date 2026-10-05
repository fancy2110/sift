/**
 * Pure tree-pruning helpers shared by the scan store.
 *
 * Kept free of Svelte/Tauri imports so the accounting and navigation logic
 * can be reasoned about (and executed) independently of the running app.
 */

/**
 * Compute the size corrections owed to surviving ancestors after pruning.
 *
 * `remove` is subtree-closed: for every connected removed component there is
 * exactly one node whose parent survives (the topmost removed node). That
 * node's last known subtree size is debited from the surviving parent and
 * every ancestor above it. Nested removed nodes are already included in the
 * component total and are therefore never debited separately — otherwise a
 * removed folder and a removed parent folder would double-count the same
 * bytes.
 *
 * Returns a map of surviving-node id -> bytes to subtract. All lookups must
 * be evaluated while the node records still exist.
 */
export function prunedSizeDeltas(
  remove: ReadonlySet<string>,
  parentOf: (id: string) => string | null | undefined,
  sizeOf: (id: string) => number | undefined
): Map<string, number> {
  const deltas = new Map<string, number>();

  const debitChain = (startParent: string, amount: number) => {
    let pid: string | null = startParent;
    while (pid !== null) {
      deltas.set(pid, (deltas.get(pid) ?? 0) + amount);
      pid = parentOf(pid) ?? null;
    }
  };

  for (const id of remove) {
    const parent = parentOf(id) ?? null;
    if (parent !== null && !remove.has(parent)) {
      debitChain(parent, sizeOf(id) ?? 0);
    }
  }
  return deltas;
}

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
