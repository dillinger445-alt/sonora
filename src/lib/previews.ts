/**
 * Soulseek previews in the interface: which files can be previewed, the
 * preview queue, how far a preview can be played, and how it is going, in
 * words. Free of React, so it can be tested on its own.
 */

import type { FoundFile, PreviewSession } from '../api/types.ts';

/** A Soulseek file to listen to before downloading it. */
export interface PreviewSource {
  username: string;
  /** The full path on the other user's computer. */
  filename: string;
  name: string;
  folder: string;
  size: number;
  /** Seconds, as the other user's client reports. */
  length: number | null;
}

/** One key per file on Soulseek. */
export const previewKey = (src: Pick<PreviewSource, 'username' | 'filename'>) => `${src.username}\n${src.filename}`;

/** A preview's place among tracks: a negative id (library tracks are positive), the same for the same file. */
export function previewTrackId(src: Pick<PreviewSource, 'username' | 'filename'>): number {
  let h = 0x811c9dc5;
  for (const ch of previewKey(src)) {
    h ^= ch.codePointAt(0) ?? 0;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return -(h + 1);
}

/** Formats a preview can be played of (decoded as they arrive). */
const PREVIEWABLE = new Set(['flac', 'wav', 'wave', 'aif', 'aiff', 'aifc']);

/** Why a file cannot be previewed, or null when it can. `local`: slskd runs on this computer. */
export function previewBlock(x: Pick<FoundFile, 'ext' | 'size'>, local: boolean): string | null {
  if (!local) return 'Previews need slskd on this computer.';
  if (x.size <= 0) return 'Its size is unknown, so it cannot be previewed.';
  const ext = x.ext.toLowerCase();
  if (PREVIEWABLE.has(ext)) return null;
  if (ext === 'm4a' || ext === 'mp4' || ext === 'alac') return 'M4A files keep what a player needs at their end, so they cannot be played before they arrive whole.';
  return `${ext ? ext.toUpperCase() : 'This file'} cannot be previewed. Previews are for FLAC, WAV and AIFF.`;
}

// ------------------------------------------------------------ the queue

/** Adds `src` at the end, unless it is queued already (then the queue is returned as it is). */
export function enqueue(q: PreviewSource[], src: PreviewSource): PreviewSource[] {
  return q.some((x) => previewKey(x) === previewKey(src)) ? q : [...q, src];
}

/** Puts `src` first (moving it when it is queued already). */
export function putFirst(q: readonly PreviewSource[], src: PreviewSource): PreviewSource[] {
  return [src, ...q.filter((x) => previewKey(x) !== previewKey(src))];
}

/** Takes `src` out. */
export function without(q: readonly PreviewSource[], src: PreviewSource): PreviewSource[] {
  return q.filter((x) => previewKey(x) !== previewKey(src));
}

/** The preview to play next and the queue after it, or null when the queue is empty. */
export function takeNext(q: readonly PreviewSource[]): { next: PreviewSource; rest: PreviewSource[] } | null {
  const [next, ...rest] = q;
  return next ? { next, rest } : null;
}

// ------------------------------------------------------------ playing

/**
 * How far a preview can be played or sought to now, in seconds: what has
 * been decoded (less a little, so the element never runs into the end of
 * what is there), or all of it once the whole file has arrived.
 */
export function playableUpTo(p: PreviewSession | null): number {
  if (p?.transfer === 'complete') return Number.POSITIVE_INFINITY;
  return Math.max(0, (p?.decodedSeconds ?? 0) - 0.3);
}

/** How a preview is going, for the player bar. */
export function previewWords(p: PreviewSession | null, username: string): string {
  if (!p) return `Asking ${username}…`;
  if (p.handedOver) return 'Downloading; it joins your library when it finishes';
  if (p.followsDownload) return 'Playing along with the download';
  if (p.limitReached) return 'The preview stops here: its cache is full';
  if (p.endsEarly) return 'The transfer stopped, so the preview ends early';
  switch (p.state) {
    case 'connecting':
      return `Asking ${username}…`;
    case 'queued':
      return p.queue != null ? `Number ${p.queue} in ${username}’s queue` : `Waiting in ${username}’s queue`;
    case 'buffering':
      return 'Buffering…';
    case 'reconnecting':
      return 'The connection dropped; asking again…';
    case 'ready':
      return p.transfer === 'complete' ? 'All of it has arrived' : p.transfer === 'paused' ? 'Enough has arrived for now' : 'Arriving as it plays';
    case 'unavailable':
    case 'failed':
      return p.error ?? 'This preview cannot be played.';
    default:
      return '';
  }
}

/** How the preview of one search result is going, for its row (null: not the one loaded). */
export function rowWords(
  player: { status: 'loading' | 'playing' | 'paused' | 'error'; error: string | null; preview: PreviewSession | null },
  username: string,
): string {
  if (player.status === 'error') return player.error ?? 'Playback failed';
  if (player.status === 'playing') return 'Playing';
  if (player.status === 'paused' && player.preview?.ready) return 'Paused';
  return previewWords(player.preview, username);
}
