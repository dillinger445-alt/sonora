/**
 * Undo for tracks taken out of a playlist: where to put them back when the
 * playlist may have changed since.
 */

export interface Reinsertion {
  /** Tracks to insert together, in order. */
  ids: number[];
  /** Where, in the list as it is after the insertions before this one. */
  index: number;
}

/**
 * The insertions that put `removed` back where they were in `before`, given
 * the list is now `current`: each run of removed tracks goes after the
 * track that preceded it (or before the one that followed it), as long as
 * that track is still there. Tracks already back in the list stay put.
 */
export function reinsertions(before: readonly number[], removed: readonly number[], current: readonly number[]): Reinsertion[] {
  const gone = new Set(removed);
  const present = new Set(current);
  const runs: { ids: number[]; prev: number | null; next: number | null }[] = [];
  let prev: number | null = null;
  let open: { ids: number[]; prev: number | null; next: number | null } | null = null;
  for (const id of before) {
    if (present.has(id)) {
      if (open) open.next = id;
      open = null;
      prev = id;
    } else if (gone.has(id)) {
      if (!open) {
        open = { ids: [], prev, next: null };
        runs.push(open);
      }
      open.ids.push(id);
    }
    // Anything else left the playlist some other way: it is skipped.
  }
  const list = [...current];
  const out: Reinsertion[] = [];
  for (const r of runs) {
    const after = r.prev != null ? list.indexOf(r.prev) : -1;
    const beforeAt = r.next != null ? list.indexOf(r.next) : -1;
    const index = after >= 0 ? after + 1 : beforeAt >= 0 ? beforeAt : list.length;
    out.push({ ids: r.ids, index });
    list.splice(index, 0, ...r.ids);
  }
  return out;
}
