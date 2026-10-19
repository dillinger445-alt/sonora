/**
 * A playlist as a set: one block per track, as long as the track and
 * tinted by its key's place on the Camelot wheel, the tempo as a line
 * across, and at each change a mark for how the keys mix (lime: they mix,
 * amber: with care, red: they clash). The track playing shows its
 * position.
 *
 * It is a picture of the table below it, which says the same in words and
 * works from the keyboard; pointing at a block shows its details, and
 * clicking it selects the track there.
 */

import { useMemo, useState } from 'react';
import type { KeyNotation, TrackRow } from '../api/types.ts';
import { formatDuration } from '../lib/format.ts';
import { formatBpm, formatKey, keyHue } from '../lib/keys.ts';
import { formatChange, MATCH_LABEL, transition, type Transition } from '../lib/mixing.ts';
import { usePlaybackTime, usePlayer } from '../state/player.tsx';

type Row = Pick<TrackRow, 'id' | 'displayTitle' | 'artist' | 'duration' | 'djBpm' | 'djKey'>;

interface Seg {
  r: Row;
  i: number;
  x: number;
  w: number;
  t: Transition | null;
}

const H = 64;
const TOP = 6;
const BAR = 30;

function layout(rows: Row[], width: number): Seg[] {
  // Unknown lengths count as four minutes; every track stays visible.
  const len = rows.map((r) => (r.duration && r.duration > 0 ? r.duration : 240));
  const total = len.reduce((a, b) => a + b, 0) || 1;
  const gap = rows.length > 1 ? 2 : 0;
  const usable = Math.max(1, width - gap * (rows.length - 1));
  const min = Math.min(4, usable / Math.max(1, rows.length));
  const raw = len.map((l) => Math.max(min, (l / total) * usable));
  const scale = usable / raw.reduce((a, b) => a + b, 0);
  let x = 0;
  return rows.map((r, i) => {
    const w = (raw[i] ?? 0) * scale;
    const seg: Seg = { r, i, x, w, t: i > 0 ? transition(rows[i - 1] as Row, r) : null };
    x += w + gap;
    return seg;
  });
}

function Playhead({ seg }: { seg: Seg }) {
  const time = usePlaybackTime();
  const d = seg.r.duration ?? 0;
  if (!(d > 0)) return null;
  const x = seg.x + Math.min(1, Math.max(0, time / d)) * seg.w;
  return <line className="tl-playhead" x1={x} x2={x} y1={TOP - 3} y2={TOP + BAR + 3} />;
}

export function SetTimeline({
  rows,
  notation,
  width,
  selected,
  onPick,
  compact = false,
  label,
}: {
  rows: Row[];
  notation: KeyNotation;
  width: number;
  selected?: ReadonlySet<number>;
  onPick?: (id: number, index: number) => void;
  compact?: boolean;
  label: string;
}) {
  const player = usePlayer();
  const [hover, setHover] = useState<number | null>(null);
  const segs = useMemo(() => layout(rows, Math.max(40, width)), [rows, width]);
  const height = compact ? BAR + TOP * 2 : H;
  const bpms = rows.map((r) => r.djBpm).filter((b): b is number => b != null && b > 0);
  const lo = bpms.length ? Math.min(...bpms) : 0;
  const hi = bpms.length ? Math.max(...bpms) : 0;
  const span = Math.max(4, hi - lo);
  // The tempo line runs below the key labels.
  const lineTop = compact ? TOP + 6 : TOP + 14;
  const lineBottom = TOP + BAR - 4;
  const yOf = (b: number) => lineBottom - ((b - lo + (span - (hi - lo)) / 2) / span) * (lineBottom - lineTop);
  const line = segs
    .filter((s) => s.r.djBpm != null && s.r.djBpm > 0)
    .map((s) => `${(s.x + s.w / 2).toFixed(1)},${yOf(s.r.djBpm as number).toFixed(1)}`)
    .join(' ');
  const playing = player.track ? segs.find((s) => s.r.id === player.track?.id) : undefined;
  const h = hover != null ? segs[hover] : undefined;

  return (
    <div className={`tl ${compact ? 'is-compact' : ''}`} style={{ width }}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label={label}
        onMouseLeave={() => setHover(null)}
      >
        {segs.map((s) => {
          const k = s.r.djKey;
          const hue = k != null ? keyHue(k) : null;
          const isSel = selected?.has(s.r.id);
          return (
            <g
              key={`${s.r.id}-${s.i}`}
              className={`tl-seg ${isSel ? 'is-selected' : ''} ${hover === s.i ? 'is-hover' : ''}`}
              onMouseEnter={() => setHover(s.i)}
              onClick={() => onPick?.(s.r.id, s.i)}
            >
              <rect
                x={s.x}
                y={TOP}
                width={Math.max(1, s.w)}
                height={BAR}
                rx={2}
                style={{ fill: hue != null ? `hsl(${hue} 30% 46% / 0.26)` : 'var(--c-unlit)', stroke: isSel ? 'var(--c-text)' : 'none' }}
              />
              <rect x={s.x} y={TOP} width={Math.max(1, s.w)} height={3} rx={1} style={{ fill: hue != null ? `hsl(${hue} 36% 64%)` : 'var(--c-text-off)' }} />
              {!compact && k != null && s.w >= 30 ? (
                <text className="tl-key" x={s.x + 5} y={TOP + 13}>
                  {formatKey(k, notation)}
                </text>
              ) : null}
            </g>
          );
        })}
        {line ? <polyline className="tl-bpm" points={line} /> : null}
        {segs.map((s) =>
          s.t ? <rect key={`m${s.i}`} className={`tl-mark is-${s.t.tone}`} x={s.x - 4} y={TOP + BAR + 3} width={6} height={6} rx={1} transform={`rotate(45 ${s.x - 1} ${TOP + BAR + 6})`} /> : null,
        )}
        {playing ? <Playhead seg={playing} /> : null}
      </svg>
      {!compact && h ? (
        <div className="tl-tip" style={{ left: Math.min(Math.max(0, h.x + h.w / 2 - 130), Math.max(0, width - 260)) }} aria-hidden>
          <span className="tl-tip-title ellipsis">
            <span className="num faint">{h.i + 1}.</span> {h.r.displayTitle}
          </span>
          <span className="faint">
            {[h.r.djBpm != null ? `${formatBpm(h.r.djBpm)} BPM` : null, h.r.djKey != null ? formatKey(h.r.djKey, notation) : null, h.r.duration ? formatDuration(h.r.duration) : null].filter(Boolean).join(', ')}
          </span>
          {h.t ? (
            <span className={`tl-tip-mix is-${h.t.tone}`}>
              From the track before: {MATCH_LABEL[h.t.match].toLowerCase()}
              {h.t.tempo ? `, ${formatChange(h.t.tempo.pct)}${h.t.tempo.halfDouble ? ' at half or double time' : ''}` : ''}
            </span>
          ) : null}
        </div>
      ) : null}
      {!compact && bpms.length ? (
        <span className="tl-range num faint" aria-hidden>
          {formatBpm(lo) === formatBpm(hi) ? `${formatBpm(lo)} BPM` : `${formatBpm(lo)}–${formatBpm(hi)} BPM`}
        </span>
      ) : null}
    </div>
  );
}
