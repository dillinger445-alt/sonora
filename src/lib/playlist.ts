/**
 * A YouTube playlist run in words: how each track and the run as a whole
 * are going, which rows a filter shows, and what a candidate file is.
 * Free of React, so it can be tested on its own.
 */

import type { PlaylistCandidate, PlaylistItem, PlaylistRun } from '../api/types.ts';
import { formatClock, formatKbps, formatRateKhz, plural } from './format.ts';

export type Filter = 'all' | 'found' | 'downloading' | 'done' | 'not_found' | 'failed' | 'left_out';

export const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'found', label: 'Found' },
  { id: 'downloading', label: 'Downloading' },
  { id: 'done', label: 'Done' },
  { id: 'not_found', label: 'Not found' },
  { id: 'failed', label: 'Failed' },
  { id: 'left_out', label: 'Left out' },
];

export function matches(i: PlaylistItem, f: Filter): boolean {
  switch (f) {
    case 'all':
      return true;
    case 'left_out':
      return !i.included;
    case 'downloading':
      return i.included && (i.status === 'downloading' || i.status === 'verifying');
    default:
      return i.included && i.status === f;
  }
}

export const trackName = (i: PlaylistItem) => (i.artist ? `${i.artist} – ${i.title}` : i.title);

/** What the other person's client says about a file. */
export function quality(c: PlaylistCandidate): string {
  if (c.lossless) {
    const parts = [c.bitDepth ? `${c.bitDepth}-bit` : null, c.sampleRate ? `${formatRateKhz(c.sampleRate)} kHz` : null].filter(Boolean);
    return parts.length ? parts.join(' / ') : 'resolution not reported';
  }
  return c.bitRate ? `${formatKbps(c.bitRate)} kbps` : 'bit rate not reported';
}

export function pickOf(i: PlaylistItem): PlaylistCandidate | null {
  const n = i.chosen ?? 0;
  const c = i.candidates[n] ?? null;
  // A file the rules turned down is no pick (B53).
  return c && c.rejected == null ? c : null;
}

/** Whether a video can be put in or left out now: in a draft, and in a run
 *  that will still search it (B17); tracks not found or failed can be
 *  looked for again. */
export function canInclude(i: PlaylistItem, runState: PlaylistRun['state']): boolean {
  if (runState === 'draft' || i.status === 'not_found' || i.status === 'failed') return true;
  return i.status === 'pending' && (runState === 'running' || runState === 'paused' || runState === 'waiting');
}

/** What search number `n` looked for (crates/sonora-core/src/matching.rs, `variations`). */
export function searchWords(n: number): string {
  return (
    [
      'the title',
      'the title and its mix name',
      'the artist and the title',
      'the artist, the title and its mix name',
      'the first artist and the title without brackets',
      'the artist alone',
      'the title’s main words',
      'the catalogue number',
      'the title, simply spelt',
      'the artist and the title, simply spelt',
    ][n - 1] ?? `search ${n}`
  );
}

/** How a track is going, in words. */
export function itemWords(i: PlaylistItem, reviewing: boolean): { text: string; tone: '' | 'faint' | 't-lime' | 't-amber' | 't-red' } {
  if (!i.included) return { text: i.leftOut ?? 'Left out', tone: 'faint' };
  switch (i.status) {
    case 'pending':
      return { text: 'Waiting to search', tone: 'faint' };
    case 'searching':
      return { text: i.variation ? `Searching (try ${i.variation})` : 'Searching', tone: '' };
    case 'found':
      return { text: reviewing ? 'Pick' : 'Found, waiting to download', tone: 't-lime' };
    case 'not_found':
      return { text: i.error ?? 'Not found', tone: 't-amber' };
    case 'downloading': {
      const t = i.transfer;
      const pct = t && t.size ? Math.floor((t.bytes / t.size) * 100) : null;
      return { text: pct != null && pct > 0 ? `Downloading ${pct}%` : 'Asked for; waiting', tone: '' };
    }
    case 'verifying':
      return { text: 'Arrived; checking it', tone: '' };
    case 'done':
      return { text: i.track?.overall != null ? `Done, scored ${Math.round(i.track.overall)}` : 'Done', tone: 't-lime' };
    case 'failed':
      return { text: i.error ?? 'Failed', tone: 't-red' };
    default:
      return { text: i.status, tone: '' };
  }
}

export function runWords(run: PlaylistRun): string {
  const c = run.counts;
  switch (run.state) {
    case 'running':
      return run.current ? `Searching for ${run.current.title}${run.current.variation ? ` (try ${run.current.variation})` : ''}` : c.downloading + c.verifying > 0 ? 'Downloading' : 'Running';
    case 'paused':
      return 'Paused: searching stops after the current track; downloads on their way go on.';
    case 'review':
      return `Searched. ${plural(c.found, 'pick')} ready to download; ${plural(c.notFound, 'track')} not found.`;
    case 'waiting':
      return `${plural(c.notFound, 'track')} not found; looking again ${run.retryAt ? `at ${formatClock(run.retryAt)}` : 'later'}.`;
    case 'finished':
      return `Done: ${plural(c.done, 'track')} downloaded and checked${c.failed ? `, ${c.failed} failed` : ''}${c.notFound ? `, ${c.notFound} not found` : ''}.`;
    case 'cancelled':
      return 'Stopped.';
    case 'failed':
      return run.error ?? 'Failed.';
    default:
      return run.state;
  }
}
