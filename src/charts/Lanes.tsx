/**
 * Time-aligned measurement lanes: three-band waveform with fault markers,
 * spectrogram with the detected cutoff, and short-term loudness, sharing
 * one time axis and one cursor. Everything drawn here comes from the
 * stored analysis; nothing is synthesized.
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import type { Visuals } from '../api/types.ts';
import { usePlaybackTime } from '../state/player.tsx';
import { formatDb, formatHz, formatTimecode } from '../lib/format.ts';
import {
  colormap,
  decodeSpectrogram,
  decodeWaveform,
  formatOverviewPeak,
  loudnessIndexAt,
  loudnessTime,
  loudnessWindow,
  spectrogramDb,
  timeTicks,
  type SpectrogramData,
  type WaveformData,
} from '../lib/visuals.ts';
import { MONO_FONT, setupCanvas, theme, useDevicePixelRatio, type Theme } from './canvas.ts';

const GUTTER = 56;
const RIGHT = 14;
const H_WAVE = 128;
const H_SPEC = 176;
const H_LOUD = 96;
const H_AXIS = 22;
const GAP = 10;

export interface LanesProps {
  duration: number;
  visuals: Visuals;
  integratedLufs: number | null;
  cutoff: { hz: number; concern: boolean } | null;
  bandwidthChanges: { time: number; fromHz: number; toHz: number }[];
  dropouts: { start: number; duration: number }[];
  /** Move the cursor here (e.g. from a finding's timestamp). */
  focus: { time: number; nonce: number } | null;
  /** This track is loaded in the player: show where playback is. */
  playhead?: boolean;
  /** Plays from a moment (a click on the lanes, or Enter at the cursor). */
  onPlayAt?: (seconds: number) => void;
}

/** The playback position over the lanes; re-renders every frame, so it stays tiny. */
function Playhead({ duration, plotW, height }: { duration: number; plotW: number; height: number }) {
  const t = usePlaybackTime();
  const x = GUTTER + (Math.max(0, Math.min(duration, t)) / duration) * plotW;
  return <div className="lanes-playhead" style={{ transform: `translateX(${x}px)`, height }} aria-hidden />;
}

function bucketRange(px: number, plotW: number, n: number): [number, number] {
  const b0 = Math.floor((px / plotW) * n);
  const b1 = Math.max(b0 + 1, Math.floor(((px + 1) / plotW) * n));
  return [Math.min(b0, n - 1), Math.min(b1, n)];
}

function drawWave(ctx: CanvasRenderingContext2D, w: WaveformData, t: Theme, plotW: number, h: number, duration: number, v: Visuals, dropouts: LanesProps['dropouts']) {
  const n = w.count;
  const mid = h / 2;
  const amp = h / 2 - 3;
  const toX = (sec: number) => GUTTER + (sec / duration) * plotW;

  // Silent regions behind everything.
  ctx.fillStyle = 'rgba(255,255,255,0.025)';
  for (const [s, e] of v.silentRegions) ctx.fillRect(toX(s), 0, Math.max(1, toX(e) - toX(s)), h);

  // Centre line and full-scale guides.
  ctx.fillStyle = t.grid;
  ctx.fillRect(GUTTER, Math.round(mid), plotW, 1);
  ctx.fillRect(GUTTER, 2, plotW, 1);
  ctx.fillRect(GUTTER, h - 3, plotW, 1);

  if (n > 0) {
    // Peak envelope.
    ctx.fillStyle = t.envelope;
    for (let px = 0; px < plotW; px += 1) {
      const [b0, b1] = bucketRange(px, plotW, n);
      let mn = 1;
      let mx = -1;
      for (let b = b0; b < b1; b += 1) {
        mn = Math.min(mn, w.min[b] ?? 0);
        mx = Math.max(mx, w.max[b] ?? 0);
      }
      if (mx < mn) continue;
      ctx.fillRect(GUTTER + px, mid - mx * amp, 1, Math.max(1, (mx - mn) * amp));
    }
    // Band RMS, largest first, so every band stays visible.
    const colors = [t.low, t.mid, t.high];
    const k = Math.SQRT2;
    for (let px = 0; px < plotW; px += 1) {
      const [b0, b1] = bucketRange(px, plotW, n);
      const vals = [0, 0, 0];
      for (let b = b0; b < b1; b += 1) {
        vals[0] = Math.max(vals[0] as number, w.low[b] ?? 0);
        vals[1] = Math.max(vals[1] as number, w.mid[b] ?? 0);
        vals[2] = Math.max(vals[2] as number, w.high[b] ?? 0);
      }
      const order = [0, 1, 2].sort((a, b) => (vals[b] as number) - (vals[a] as number));
      for (const i of order) {
        const a = Math.min(1, (vals[i] as number) * k);
        if (a <= 0.002) continue;
        ctx.fillStyle = colors[i] as string;
        ctx.fillRect(GUTTER + px, mid - a * amp, 1, Math.max(1, 2 * a * amp));
      }
    }
  }

  // Faults: clipping (red) and true-peak overs (amber) at the top edge, dropouts at the bottom.
  ctx.fillStyle = t.amber;
  for (const s of v.overTimes) ctx.fillRect(Math.round(toX(s)), 0, 1, 7);
  ctx.fillStyle = t.red;
  for (const s of v.clipTimes) {
    const x = Math.round(toX(s));
    ctx.fillRect(x, 0, 1, 9);
    ctx.fillRect(x, h - 4, 1, 4);
  }
  for (const d of dropouts) {
    const x = Math.round(toX(d.start));
    ctx.fillRect(x - 1, h - 9, 3, 9);
  }
}

function drawSpec(ctx: CanvasRenderingContext2D, s: SpectrogramData, t: Theme, plotW: number, h: number, duration: number, props: LanesProps) {
  const off = document.createElement('canvas');
  off.width = Math.max(1, s.cols);
  off.height = Math.max(1, s.rows);
  const octx = off.getContext('2d');
  if (octx && s.cols > 0) {
    const img = octx.createImageData(s.cols, s.rows);
    const lut = colormap();
    for (let c = 0; c < s.cols; c += 1) {
      for (let r = 0; r < s.rows; r += 1) {
        const b = s.data[c * s.rows + r] ?? 0;
        const y = s.rows - 1 - r;
        const o = (y * s.cols + c) * 4;
        img.data[o] = lut[b * 4] as number;
        img.data[o + 1] = lut[b * 4 + 1] as number;
        img.data[o + 2] = lut[b * 4 + 2] as number;
        img.data[o + 3] = 255;
      }
    }
    octx.putImageData(img, 0, 0);
    ctx.imageSmoothingEnabled = true;
    // The spectrogram spans the analysed audio; map its columns onto time.
    const spanSecs = s.cols * s.secsPerCol;
    const drawW = Math.min(plotW, (spanSecs / duration) * plotW);
    ctx.drawImage(off, GUTTER, 0, drawW, h);
  }

  // Frequency grid.
  const maxHz = s.maxHz;
  const step = maxHz > 30000 ? 10000 : 5000;
  ctx.font = MONO_FONT;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  for (let f = 0; f <= maxHz; f += step) {
    const y = h - (f / maxHz) * h;
    ctx.fillStyle = 'rgba(20,22,25,0.55)';
    if (f > 0) ctx.fillRect(GUTTER, Math.round(y), plotW, 1);
    if (f === 0) continue;
    ctx.fillStyle = t.text3;
    ctx.fillText(`${f / 1000}k`, GUTTER - 8, Math.max(6, y));
  }

  // Detected cutoff.
  if (props.cutoff && props.cutoff.hz < maxHz) {
    const y = Math.round(h - (props.cutoff.hz / maxHz) * h) + 0.5;
    ctx.strokeStyle = props.cutoff.concern ? t.amber : t.text2;
    ctx.setLineDash([4, 3]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(GUTTER, y);
    ctx.lineTo(GUTTER + plotW, y);
    ctx.stroke();
    ctx.setLineDash([]);
    const label = `cutoff ${formatHz(props.cutoff.hz)}`;
    ctx.font = MONO_FONT;
    const tw = ctx.measureText(label).width;
    ctx.fillStyle = 'rgba(20,22,25,0.85)';
    ctx.fillRect(GUTTER + plotW - tw - 12, y - 16, tw + 8, 13);
    ctx.fillStyle = props.cutoff.concern ? t.amber : t.text;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'alphabetic';
    ctx.fillText(label, GUTTER + plotW - 8, y - 6);
  }

  // Bandwidth changes over time.
  if (props.bandwidthChanges.length) {
    ctx.strokeStyle = t.amber;
    ctx.setLineDash([2, 3]);
    for (const c of props.bandwidthChanges) {
      const x = Math.round(GUTTER + (c.time / duration) * plotW) + 0.5;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
    }
    ctx.setLineDash([]);
  }
}

function loudRange(values: (number | null)[]): [number, number] {
  let mn = Infinity;
  for (const v of values) if (v != null && v < mn) mn = v;
  const lo = Number.isFinite(mn) ? Math.min(-30, Math.floor((mn - 2) / 10) * 10) : -40;
  return [Math.max(lo, -70), 0];
}

function drawLoud(ctx: CanvasRenderingContext2D, v: Visuals, t: Theme, plotW: number, h: number, duration: number, integrated: number | null) {
  const [lo, hi] = loudRange(v.loudness.values);
  const toY = (l: number) => 4 + (1 - (l - lo) / (hi - lo)) * (h - 8);
  ctx.font = MONO_FONT;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  for (let l = hi; l >= lo; l -= 10) {
    const y = Math.round(toY(l));
    ctx.fillStyle = t.grid;
    ctx.fillRect(GUTTER, y, plotW, 1);
    ctx.fillStyle = t.text3;
    ctx.fillText(String(l), GUTTER - 8, y);
  }
  const step = v.loudness.step;
  const win = loudnessWindow(v.loudness, duration);
  const vals = v.loudness.values;
  // Area and line, broken where the signal is below the gate. Each value is
  // drawn at the end of the window it measures, in line with the waveform.
  ctx.beginPath();
  let open = false;
  let startX = 0;
  let lastX = 0;
  const fills: [number, number, number[]][] = [];
  let seg: number[] = [];
  for (let i = 0; i < vals.length; i += 1) {
    const val = vals[i];
    const x = GUTTER + (Math.min(duration, loudnessTime(i, step, win)) / duration) * plotW;
    if (val == null) {
      if (open) fills.push([startX, lastX, seg]);
      open = false;
      seg = [];
      continue;
    }
    const y = toY(Math.max(lo, Math.min(hi, val)));
    if (!open) {
      ctx.moveTo(x, y);
      startX = x;
      open = true;
    } else ctx.lineTo(x, y);
    seg.push(x, y);
    lastX = x;
  }
  if (open) fills.push([startX, lastX, seg]);
  for (const [, , pts] of fills) {
    if (pts.length < 4) continue;
    ctx.save();
    const g = ctx.createLinearGradient(0, 0, 0, h);
    g.addColorStop(0, 'rgba(159,183,204,0.22)');
    g.addColorStop(1, 'rgba(159,183,204,0.02)');
    ctx.fillStyle = g;
    const p = new Path2D();
    p.moveTo(pts[0] as number, h);
    for (let i = 0; i < pts.length; i += 2) p.lineTo(pts[i] as number, pts[i + 1] as number);
    p.lineTo(pts[pts.length - 2] as number, h);
    p.closePath();
    ctx.fill(p);
    ctx.restore();
  }
  ctx.strokeStyle = t.data1;
  ctx.lineWidth = 1.25;
  ctx.stroke();

  if (integrated != null && integrated > lo) {
    const y = Math.round(toY(integrated)) + 0.5;
    ctx.strokeStyle = t.text2;
    ctx.setLineDash([5, 4]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(GUTTER, y);
    ctx.lineTo(GUTTER + plotW, y);
    ctx.stroke();
    ctx.setLineDash([]);
    const label = `integrated ${formatDb(integrated)} LUFS`;
    const tw = ctx.measureText(label).width;
    const above = y > 20;
    ctx.fillStyle = 'rgba(20,22,25,0.85)';
    ctx.fillRect(GUTTER + plotW - tw - 12, above ? y - 15 : y + 3, tw + 8, 13);
    ctx.fillStyle = t.text2;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'alphabetic';
    ctx.fillText(label, GUTTER + plotW - 8, above ? y - 5 : y + 13);
  }
}

function drawAxis(ctx: CanvasRenderingContext2D, t: Theme, plotW: number, duration: number) {
  ctx.font = MONO_FONT;
  ctx.textBaseline = 'top';
  ctx.fillStyle = t.line;
  ctx.fillRect(GUTTER, 0, plotW, 1);
  const ticks = timeTicks(duration, Math.max(2, Math.floor(plotW / 90)));
  for (const s of ticks) {
    const x = Math.round(GUTTER + (s / duration) * plotW);
    ctx.fillStyle = t.line;
    ctx.fillRect(x, 0, 1, 4);
    ctx.fillStyle = t.text3;
    ctx.textAlign = s === 0 ? 'left' : x > GUTTER + plotW - 20 ? 'right' : 'center';
    ctx.fillText(formatTimecode(s), x, 7);
  }
}

interface Readout {
  time: number;
  /** Linear peak of the waveform overview at the cursor. */
  peak: number | null;
  rmsDb: number | null;
  /** `undefined` before the first full loudness window; `null` when gated. */
  lufs: number | null | undefined;
  spec: { hz: number; db: number } | null;
}

/** Overview values are stored at 8-bit precision: whole decibels only. */
function rmsText(db: number | null): string {
  if (db == null) return '—';
  return db > -89.9 ? `${formatDb(db, 0)} dBFS` : 'below -90 dBFS';
}

function lufsText(l: number | null | undefined): string {
  if (l === undefined) return '—';
  return l == null ? 'below the gate' : `${formatDb(l)} LUFS`;
}

export function Lanes(props: LanesProps) {
  const { duration, visuals, integratedLufs } = props;
  const wrapRef = useRef<HTMLDivElement>(null);
  const waveRef = useRef<HTMLCanvasElement>(null);
  const specRef = useRef<HTMLCanvasElement>(null);
  const loudRef = useRef<HTMLCanvasElement>(null);
  const axisRef = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  const [cursor, setCursor] = useState<{ t: number; specY: number | null } | null>(null);
  const [announce, setAnnounce] = useState('');

  const wave = useMemo(() => decodeWaveform(visuals.waveform), [visuals.waveform]);
  const spec = useMemo(() => decodeSpectrogram(visuals.spectrogram), [visuals.spectrogram]);
  const plotW = Math.max(10, width - GUTTER - RIGHT);
  const dpr = useDevicePixelRatio();

  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (width <= 0 || !(duration > 0)) return;
    const t = theme();
    const draw = (c: HTMLCanvasElement | null, h: number, fn: (ctx: CanvasRenderingContext2D) => void) => {
      if (!c) return;
      const ctx = setupCanvas(c, width, h);
      if (ctx) fn(ctx);
    };
    draw(waveRef.current, H_WAVE, (ctx) => drawWave(ctx, wave, t, plotW, H_WAVE, duration, visuals, props.dropouts));
    draw(specRef.current, H_SPEC, (ctx) => drawSpec(ctx, spec, t, plotW, H_SPEC, duration, props));
    draw(loudRef.current, H_LOUD, (ctx) => drawLoud(ctx, visuals, t, plotW, H_LOUD, duration, integratedLufs));
    draw(axisRef.current, H_AXIS, (ctx) => drawAxis(ctx, t, plotW, duration));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [width, plotW, dpr, duration, wave, spec, visuals, integratedLufs, props.cutoff, props.bandwidthChanges, props.dropouts]);

  const readout = useCallback(
    (time: number, specY: number | null): Readout => {
      const b = Math.min(wave.count - 1, Math.max(0, Math.floor(time / wave.bucketSecs)));
      const peak = wave.count > 0 ? Math.max(Math.abs(wave.min[b] ?? 0), Math.abs(wave.max[b] ?? 0)) : null;
      const loud = visuals.loudness;
      const li = loudnessIndexAt(time, loud.step, loudnessWindow(loud, duration), loud.values.length);
      const lufs = li < 0 ? undefined : (loud.values[li] ?? null);
      let specOut: Readout['spec'] = null;
      if (specY != null && spec.cols > 0) {
        const col = Math.min(spec.cols - 1, Math.max(0, Math.floor(time / spec.secsPerCol)));
        const row = Math.min(spec.rows - 1, Math.max(0, Math.floor((1 - specY / H_SPEC) * spec.rows)));
        specOut = { hz: ((row + 0.5) / spec.rows) * spec.maxHz, db: spectrogramDb(spec, col, row) };
      }
      return {
        time,
        peak,
        rmsDb: wave.count > 0 ? (wave.rmsDb[b] ?? null) : null,
        lufs,
        spec: specOut,
      };
    },
    [wave, spec, visuals.loudness, duration],
  );

  // Jump to a time requested from outside (finding timestamps).
  useEffect(() => {
    if (!props.focus) return;
    setCursor({ t: Math.max(0, Math.min(duration, props.focus.time)), specY: null });
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    wrapRef.current?.scrollIntoView({ block: 'nearest', behavior: reduced ? 'auto' : 'smooth' });
    wrapRef.current?.focus({ preventScroll: true });
  }, [props.focus, duration]);

  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - r.left - GUTTER;
    if (x < 0 || x > plotW) {
      setCursor(null);
      return;
    }
    const y = e.clientY - r.top;
    const specTop = H_WAVE + GAP;
    const specY = y >= specTop && y <= specTop + H_SPEC ? y - specTop : null;
    setCursor({ t: (x / plotW) * duration, specY });
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = duration / (e.shiftKey ? 20 : 100);
    let t = cursor?.t ?? 0;
    if (e.key === 'ArrowRight') t = Math.min(duration, t + step);
    else if (e.key === 'ArrowLeft') t = Math.max(0, t - step);
    else if (e.key === 'Home') t = 0;
    else if (e.key === 'End') t = duration;
    else if (e.key === 'Enter' && props.onPlayAt) {
      e.preventDefault();
      props.onPlayAt(cursor?.t ?? 0);
      return;
    } else if (e.key === 'Escape') {
      setCursor(null);
      return;
    } else return;
    e.preventDefault();
    setCursor({ t, specY: null });
    const ro = readout(t, null);
    setAnnounce(`${formatTimecode(t)}: peak ${ro.peak != null ? formatOverviewPeak(ro.peak) : 'not available'}, short-term loudness ${lufsText(ro.lufs)}`);
  };

  const ro = cursor ? readout(cursor.t, cursor.specY) : null;
  const cx = cursor ? GUTTER + (cursor.t / duration) * plotW : 0;
  const flip = cx > width - 200;
  const totalH = H_WAVE + GAP + H_SPEC + GAP + H_LOUD + H_AXIS;

  return (
    <div
      ref={wrapRef}
      tabIndex={0}
      role="group"
      aria-label={`Waveform, spectrogram and loudness over time. Use the left and right arrow keys to move the cursor and hear the values at that point.${props.onPlayAt ? ' Press Enter, or click, to play from there.' : ''}`}
      onPointerMove={onMove}
      onPointerLeave={() => setCursor(null)}
      onKeyDown={onKey}
      onClick={(e) => {
        if (!props.onPlayAt) return;
        const r = e.currentTarget.getBoundingClientRect();
        const x = e.clientX - r.left - GUTTER;
        if (x < 0 || x > plotW) return;
        props.onPlayAt((x / plotW) * duration);
      }}
      className={`lanes ${props.onPlayAt ? 'is-playable' : ''}`}
      style={{ height: totalH }}
    >
      <div className="lane" style={{ height: H_WAVE }}>
        <canvas ref={waveRef} aria-hidden />
        <div className="lane-label">
          Waveform
          <span className="lane-legend" aria-hidden>
            <i className="lg lg-low" />
            low
            <i className="lg lg-mid" />
            mid
            <i className="lg lg-high" />
            high
          </span>
        </div>
      </div>
      <div className="lane" style={{ height: H_SPEC, marginTop: GAP }}>
        <canvas ref={specRef} aria-hidden />
        <div className="lane-label">Spectrogram</div>
      </div>
      <div className="lane" style={{ height: H_LOUD, marginTop: GAP }}>
        <canvas ref={loudRef} aria-hidden />
        <div className="lane-label">
          Loudness
          <span className="lane-unit">LUFS, short-term</span>
        </div>
      </div>
      <div className="lane" style={{ height: H_AXIS }}>
        <canvas ref={axisRef} aria-hidden />
      </div>
      {props.playhead && duration > 0 ? <Playhead duration={duration} plotW={plotW} height={totalH - H_AXIS} /> : null}
      {cursor && ro ? (
        <>
          <div className="lanes-cursor" style={{ left: cx, height: totalH - H_AXIS }} aria-hidden />
          <div className={`lanes-readout ${flip ? 'is-flipped' : ''}`} style={{ left: flip ? cx - 8 : cx + 8 }} aria-hidden>
            <div className="ro-time num">{formatTimecode(ro.time, true)}</div>
            <div className="ro-row">
              <span>Mid peak</span>
              <span className="num">{ro.peak != null ? formatOverviewPeak(ro.peak) : '—'}</span>
            </div>
            <div className="ro-row">
              <span>Mid RMS</span>
              <span className="num">{rmsText(ro.rmsDb)}</span>
            </div>
            <div className="ro-row">
              <span>Loudness</span>
              <span className="num">{lufsText(ro.lufs)}</span>
            </div>
            {ro.spec ? (
              <div className="ro-row">
                <span>{formatHz(ro.spec.hz)}</span>
                <span className="num">{formatDb(ro.spec.db, 0)} dB</span>
              </div>
            ) : null}
          </div>
        </>
      ) : null}
      <div className="visually-hidden" aria-live="polite">
        {announce}
      </div>
    </div>
  );
}
