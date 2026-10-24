/**
 * Rows fetched again keep the objects of the rows they replace when nothing
 * in them changed, so the table's memoised rows skip rendering (a scan
 * refreshes the library every second or so, mostly with the same tracks).
 */
export function reuseRows<T extends { id: number }>(before: readonly T[], fresh: T[]): T[] {
  if (before.length === 0) return fresh;
  const byId = new Map<number, T>();
  for (const r of before) byId.set(r.id, r);
  let reused = 0;
  const out = fresh.map((r) => {
    const old = byId.get(r.id);
    if (old !== undefined && old !== r && JSON.stringify(old) === JSON.stringify(r)) {
      reused += 1;
      return old;
    }
    return r;
  });
  return reused > 0 ? out : fresh;
}
