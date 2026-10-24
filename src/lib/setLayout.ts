/**
 * Where each track of a set goes on the timeline (`SetTimeline`), and what
 * the pointer is over. Blocks are as long as their tracks; every track
 * stays visible, however short.
 */

import type { TrackRow } from '../api/types.ts';
import { transition, type Transition } from './mixing.ts';

export type SetRow = Pick<TrackRow, 'id' | 'displayTitle' | 'artist' | 'duration' | 'djBpm' | 'djKey'>;

export interface Seg<R extends SetRow = SetRow> {
  r: R;
  i: number;
  x: number;
  w: number;
  /** From the track before (none for the first). */
  t: Transition | null;
}

export function layoutSet<R extends SetRow>(rows: R[], width: number): Seg<R>[] {
  const n = rows.length;
  // Unknown lengths count as four minutes.
  const len = rows.map((r) => (r.duration && r.duration > 0 ? r.duration : 240));
  const total = len.reduce((a, b) => a + b, 0) || 1;
  // Gaps between the blocks while each block keeps two pixels or more; a
  // longer set runs block to block (with gaps, it would not fit the width).
  const gap = n > 1 && width - 2 * (n - 1) >= 2 * n ? 2 : 0;
  const usable = Math.max(1, width - gap * (n - 1));
  const min = Math.min(4, usable / Math.max(1, n));
  const raw = len.map((l) => Math.max(min, (l / total) * usable));
  const scale = usable / raw.reduce((a, b) => a + b, 0);
  let x = 0;
  return rows.map((r, i) => {
    const w = (raw[i] ?? 0) * scale;
    const seg: Seg<R> = { r, i, x, w, t: i > 0 ? transition(rows[i - 1] as R, r) : null };
    x += w + gap;
    return seg;
  });
}

/** The block at `x` (or the one whose gap it is in); none before the first or past the last. */
export function segmentAt(segs: readonly Seg[], x: number): number | null {
  if (!segs.length || x < 0) return null;
  let lo = 0;
  let hi = segs.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if ((segs[mid] as Seg).x <= x) lo = mid;
    else hi = mid - 1;
  }
  const s = segs[lo] as Seg;
  const next = segs[lo + 1];
  const end = next ? next.x : s.x + Math.max(1, s.w);
  return x <= end ? lo : null;
}

/**
 * A set too long for a block per track: one column per pixel, coloured by
 * the key of the track under its middle, neighbours of one key merged.
 */
export function columnsOf(segs: readonly Seg[], width: number): { x: number; w: number; key: number | null }[] {
  const out: { x: number; w: number; key: number | null }[] = [];
  let j = 0;
  const end = segs.length ? Math.min(Math.ceil(width), Math.ceil((segs.at(-1) as Seg).x + (segs.at(-1) as Seg).w)) : 0;
  for (let px = 0; px < end; px += 1) {
    const mid = px + 0.5;
    while (j + 1 < segs.length && (segs[j + 1] as Seg).x <= mid) j += 1;
    const key = (segs[j] as Seg).r.djKey ?? null;
    const last = out.at(-1);
    if (last && last.key === key && last.x + last.w === px) last.w += 1;
    else out.push({ x: px, w: 1, key });
  }
  return out;
}

/** Outlines for the selected tracks: one per block, or per run of neighbours in a long set. */
export function selectedRuns(segs: readonly Seg[], selected: ReadonlySet<number>, merge: boolean): { x: number; w: number }[] {
  const out: { x: number; w: number }[] = [];
  let open: { x: number; w: number } | null = null;
  for (const s of segs) {
    if (!selected.has(s.r.id)) {
      open = null;
      continue;
    }
    if (merge && open) {
      open.w = s.x + s.w - open.x;
    } else {
      open = { x: s.x, w: s.w };
      out.push(open);
    }
  }
  return out;
}
