/** Small marks for tempo, key and genre, and where each value came from. */

import type { DjSource, KeyNotation } from '../api/types.ts';
import { formatBpm, formatKey, keyColor, longKeyName } from '../lib/keys.ts';

export const SOURCE_LABEL: Record<DjSource, string> = {
  edit: 'Edited',
  detected: 'Detected',
  tag: 'From tags',
  folder: 'From folder',
  discogs: 'Discogs',
};

export const SOURCE_HINT: Record<DjSource, string> = {
  edit: 'Set by you in SONORA (the file is not changed)',
  detected: 'Detected by SONORA from the audio',
  tag: "From the file's own tags",
  folder: 'From the name of the folder the file is in',
  discogs: 'Looked up on Discogs by artist and title',
};

/** A key in the chosen notation, with its colour on the wheel. */
export function KeyBadge({ keyId, notation, size = 'sm' }: { keyId: number | null | undefined; notation: KeyNotation; size?: 'sm' | 'lg' }) {
  if (keyId == null) return null;
  return (
    <span className={`key-badge ${size === 'lg' ? 'is-lg' : ''}`} data-tip={longKeyName(keyId)}>
      <i className="key-dot" style={{ background: keyColor(keyId) }} aria-hidden />
      <span className="key-code">{formatKey(keyId, notation)}</span>
    </span>
  );
}

export function BpmText({ bpm, variable }: { bpm: number | null | undefined; variable?: boolean | null }) {
  if (bpm == null) return null;
  return (
    <span className="num bpm-text" data-tip={variable ? 'The tempo changes or drifts in this track' : undefined}>
      {formatBpm(bpm)}
      {variable ? (
        <>
          <span className="bpm-var" aria-hidden>
            ~
          </span>
          <span className="visually-hidden"> (variable tempo)</span>
        </>
      ) : null}
    </span>
  );
}

export function SourceNote({ source }: { source: DjSource | null | undefined }) {
  if (!source) return null;
  return (
    <span className={`dj-source dj-source-${source}`} data-tip={SOURCE_HINT[source]}>
      {SOURCE_LABEL[source]}
    </span>
  );
}
