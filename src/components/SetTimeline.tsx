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
 *
 * Drawn for long sets too: the blocks are drawn once and kept while the
 * selection, the pointer and the playhead change on top of them, and when
 * a track gets less than two pixels, each pixel shows the track under it
 * (a set of thousands of tracks is a strip of colour, not thousands of
 * shapes).
 */

import { useMemo, useState, type MouseEvent } from 'react';
import type { KeyNotation, TrackRow } from '../api/types.ts';
import { formatDuration } from '../lib/format.ts';
import { formatBpm, formatKey, keyHue } from '../lib/keys.ts';
import { formatChange, MATCH_LABEL } from '../lib/mixing.ts';
import { columnsOf, layoutSet, segmentAt, selectedRuns, type Seg, type SetRow } from '../lib/setLayout.ts';
import { usePlaybackTime, usePlayer } from '../state/player.tsx';

type Row = Pick<TrackRow, 'id' | 'displayTitle' | 'artist' | 'duration' | 'djBpm' | 'djKey'>;

const H = 64;
const TOP = 6;
const BAR = 30;

const bodyFill = (k: number | null | undefined) => (k != null ? `hsl(${keyHue(k)} 30% 46% / 0.26)` : 'var(--c-unlit)');
const stripeFill = (k: number | null | undefined) => (k != null ? `hsl(${keyHue(k)} 36% 64%)` : 'var(--c-text-off)');

function Playhead({ seg }: { seg: Seg<SetRow> }) {
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
  const w = Math.max(40, width);
  const segs = useMemo(() => layoutSet(rows, w), [rows, w]);
  // Under two pixels a track: one column of colour per pixel.
  const dense = rows.length * 2 > w;
  const height = compact ? BAR + TOP * 2 : H;

  const { lo, hi } = useMemo(() => {
    const bpms = rows.map((r) => r.djBpm).filter((b): b is number => b != null && b > 0);
    return bpms.length ? { lo: Math.min(...bpms), hi: Math.max(...bpms) } : { lo: 0, hi: 0 };
  }, [rows]);
  const hasBpm = hi > 0;

  // The blocks, marks and tempo line: drawn again only when the set or the
  // width changes, not on every selection or pointer move.
  const base = useMemo(() => {
    const span = Math.max(4, hi - lo);
    const lineTop = compact ? TOP + 6 : TOP + 14;
    const lineBottom = TOP + BAR - 4;
    const yOf = (b: number) => lineBottom - ((b - lo + (span - (hi - lo)) / 2) / span) * (lineBottom - lineTop);
    const line = segs
      .filter((s) => s.r.djBpm != null && s.r.djBpm > 0)
      .map((s) => `${(s.x + s.w / 2).toFixed(1)},${yOf(s.r.djBpm as number).toFixed(1)}`)
      .join(' ');
    return (
      <>
        <g className="tl-segs">
          {dense
            ? columnsOf(segs, w).map((c) => (
                <g key={c.x}>
                  <rect x={c.x} y={TOP} width={c.w} height={BAR} style={{ fill: bodyFill(c.key) }} />
                  <rect x={c.x} y={TOP} width={c.w} height={3} style={{ fill: stripeFill(c.key) }} />
                </g>
              ))
            : segs.map((s) => (
                <g key={`${s.r.id}-${s.i}`}>
                  <rect x={s.x} y={TOP} width={Math.max(1, s.w)} height={BAR} rx={2} style={{ fill: bodyFill(s.r.djKey) }} />
                  <rect x={s.x} y={TOP} width={Math.max(1, s.w)} height={3} rx={1} style={{ fill: stripeFill(s.r.djKey) }} />
                  {!compact && s.r.djKey != null && s.w >= 30 ? (
                    <text className="tl-key" x={s.x + 5} y={TOP + 13}>
                      {formatKey(s.r.djKey, notation)}
                    </text>
                  ) : null}
                </g>
              ))}
        </g>
        {line ? <polyline className="tl-bpm" points={line} /> : null}
        {dense
          ? null
          : segs.map((s) =>
              s.t ? <rect key={`m${s.i}`} className={`tl-mark is-${s.t.tone}`} x={s.x - 4} y={TOP + BAR + 3} width={6} height={6} rx={1} transform={`rotate(45 ${s.x - 1} ${TOP + BAR + 6})`} /> : null,
            )}
      </>
    );
  }, [segs, dense, w, lo, hi, compact, notation]);

  const marks = useMemo(() => (selected && selected.size ? selectedRuns(segs, selected, dense) : []), [segs, selected, dense]);
  const playing = player.track ? segs.find((s) => s.r.id === player.track?.id) : undefined;
  const h = hover != null ? segs[hover] : undefined;

  const at = (e: MouseEvent<SVGSVGElement>) => segmentAt(segs, e.clientX - e.currentTarget.getBoundingClientRect().left);

  return (
    <div className={`tl ${compact ? 'is-compact' : ''}`} style={{ width }}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label={label}
        onMouseMove={(e) => {
          const i = at(e);
          if (i !== hover) setHover(i);
        }}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const i = at(e);
          const s = i != null ? segs[i] : undefined;
          if (s) onPick?.(s.r.id, s.i);
        }}
      >
        {base}
        {h ? <rect className="tl-hover" x={h.x} y={TOP} width={Math.max(1, h.w)} height={BAR} rx={dense ? 0 : 2} style={{ fill: bodyFill(h.r.djKey) }} /> : null}
        {marks.map((m) => (
          <rect key={m.x} className="tl-selected" x={m.x} y={TOP} width={Math.max(1, m.w)} height={BAR} rx={dense ? 0 : 2} />
        ))}
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
      {!compact && hasBpm ? (
        <span className="tl-range num faint" aria-hidden>
          {formatBpm(lo) === formatBpm(hi) ? `${formatBpm(lo)} BPM` : `${formatBpm(lo)}–${formatBpm(hi)} BPM`}
        </span>
      ) : null}
    </div>
  );
}
