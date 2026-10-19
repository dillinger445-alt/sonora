/**
 * The transport strip at the bottom of the window while a track is loaded:
 * play/pause, the track's waveform overview as a seek bar, time, tempo and
 * key, volume. The waveform comes from the stored analysis (nothing is
 * decoded for it); without one the bar is a plain line.
 */

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { LuLoaderCircle, LuPause, LuPlay, LuVolume2, LuVolumeX, LuX } from 'react-icons/lu';
import { formatTimecode } from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import { decodeWaveform, type WaveformData } from '../lib/visuals.ts';
import { useCore } from '../state/core.tsx';
import { usePlaybackTime, usePlayer } from '../state/player.tsx';
import { IconButton } from './Button.tsx';
import { cssVar, setupCanvas, theme, useDevicePixelRatio } from '../charts/canvas.ts';
import { BpmText, KeyBadge } from './DjMarks.tsx';

function drawOverview(c: HTMLCanvasElement, w: WaveformData | null, width: number, height: number, color: string) {
  const ctx = setupCanvas(c, width, height);
  if (!ctx) return;
  const mid = height / 2;
  ctx.fillStyle = color;
  if (!w || w.count === 0) {
    ctx.fillRect(0, Math.round(mid), width, 1);
    return;
  }
  for (let px = 0; px < width; px += 1) {
    const b0 = Math.floor((px / width) * w.count);
    const b1 = Math.max(b0 + 1, Math.floor(((px + 1) / width) * w.count));
    let mx = 0;
    for (let b = b0; b < Math.min(b1, w.count); b += 1) mx = Math.max(mx, Math.abs(w.min[b] ?? 0), Math.abs(w.max[b] ?? 0));
    const h = Math.max(1, mx * (height - 2));
    ctx.fillRect(px, mid - h / 2, 1, h);
  }
}

/** Waveform seek bar; re-renders on every frame, so it stays small. */
function SeekBar({ duration, wave, onSeek }: { duration: number; wave: WaveformData | null; onSeek: (t: number) => void }) {
  const time = usePlaybackTime();
  const wrap = useRef<HTMLDivElement>(null);
  const base = useRef<HTMLCanvasElement>(null);
  const played = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  const dragging = useRef(false);
  const dpr = useDevicePixelRatio();
  const height = 30;

  useLayoutEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (width <= 0) return;
    const t = theme();
    if (base.current) drawOverview(base.current, wave, width, height, cssVar('--c-text-off', '#5f666e'));
    if (played.current) drawOverview(played.current, wave, width, height, t.lime);
  }, [wave, width, dpr]);

  const at = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return (Math.max(0, Math.min(r.width, e.clientX - r.left)) / Math.max(1, r.width)) * duration;
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 30 : 5;
    let t: number | null = null;
    if (e.key === 'ArrowRight' || e.key === 'ArrowUp') t = time + step;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') t = time - step;
    else if (e.key === 'Home') t = 0;
    else if (e.key === 'End') t = duration - 0.5;
    else if (e.key === 'PageUp') t = time + 30;
    else if (e.key === 'PageDown') t = time - 30;
    if (t == null) return;
    e.preventDefault();
    onSeek(Math.max(0, Math.min(duration, t)));
  };
  const frac = duration > 0 ? Math.max(0, Math.min(1, time / duration)) : 0;

  return (
    <div
      ref={wrap}
      className="seek"
      role="slider"
      tabIndex={0}
      aria-label="Position"
      aria-valuemin={0}
      aria-valuemax={Math.round(duration)}
      aria-valuenow={Math.round(time)}
      aria-valuetext={`${formatTimecode(time)} of ${formatTimecode(duration)}`}
      onKeyDown={onKey}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
        onSeek(at(e));
      }}
      onPointerMove={(e) => {
        if (dragging.current) onSeek(at(e));
      }}
      onPointerUp={() => {
        dragging.current = false;
      }}
      onPointerCancel={() => {
        dragging.current = false;
      }}
    >
      <canvas ref={base} aria-hidden />
      <div className="seek-played" style={{ width: `${frac * 100}%` }} aria-hidden>
        <canvas ref={played} />
      </div>
      <div className="seek-head" style={{ left: `${frac * 100}%` }} aria-hidden />
    </div>
  );
}

function Clock({ duration }: { duration: number | null }) {
  const time = usePlaybackTime();
  return (
    <span className="player-time num" aria-hidden>
      {formatTimecode(time)}
      <span className="faint"> / {duration != null ? formatTimecode(duration) : '–:––'}</span>
    </span>
  );
}

export function PlayerBar() {
  const { client, settings } = useCore();
  const player = usePlayer();
  const t = player.track;
  const [wave, setWave] = useState<WaveformData | null>(null);
  const [muted, setMuted] = useState<number | null>(null);
  const id = t?.id ?? null;

  // The overview of the loaded track, from its stored analysis.
  useEffect(() => {
    setWave(null);
    if (id == null) return;
    let stale = false;
    client.track(id).then(
      (d) => {
        if (!stale && d.analysis?.visuals) setWave(decodeWaveform(d.analysis.visuals.waveform));
      },
      () => undefined,
    );
    return () => {
      stale = true;
    };
  }, [client, id]);

  if (!t) return null;
  const active = player.status === 'playing' || player.status === 'loading';
  const duration = player.duration ?? t.duration ?? null;
  const notation = settings?.keyNotation ?? 'camelot';

  return (
    <section className={`player ${player.status === 'error' ? 'has-error' : ''}`} aria-label="Player">
      <button
        type="button"
        className={`player-play ${active ? 'is-on' : ''}`}
        aria-label={active ? 'Pause' : 'Play'}
        data-tip={active ? 'Pause (P)' : 'Play (P)'}
        onClick={() => (active ? player.pause() : player.resume())}
      >
        {player.status === 'loading' ? <LuLoaderCircle className="spin" aria-hidden /> : active ? <LuPause aria-hidden /> : <LuPlay aria-hidden />}
      </button>
      <div className="player-meta">
        <button type="button" className="player-title" onClick={() => navigate({ view: 'track', id: t.id })} data-tip="Open the full analysis">
          {t.displayTitle}
        </button>
        {player.status === 'error' ? (
          <span className="player-sub t-red" role="alert">
            {player.error ?? 'This file cannot be played.'}
          </span>
        ) : (
          <span className="player-sub faint">
            {t.artist ?? ''}
            {player.transcoded ? (
              <span className="player-note" data-tip="This window cannot read the file's format directly, so SONORA plays a decoded copy kept in its cache folder.">
                {t.artist ? ' · ' : ''}decoded copy
              </span>
            ) : null}
          </span>
        )}
      </div>
      {duration != null && duration > 0 ? <SeekBar duration={duration} wave={wave} onSeek={player.seek} /> : <div className="seek is-empty" />}
      <Clock duration={duration} />
      <span className="player-dj">
        <BpmText bpm={t.djBpm} />
        {t.djBpm != null ? <span className="faint player-unit">BPM</span> : null}
        <KeyBadge keyId={t.djKey} notation={notation} />
      </span>
      <span className="player-volume">
        <IconButton
          size="sm"
          label={muted != null || player.volume === 0 ? 'Unmute' : 'Mute'}
          icon={muted != null || player.volume === 0 ? LuVolumeX : LuVolume2}
          onClick={() => {
            if (muted != null) {
              player.setVolume(muted);
              setMuted(null);
            } else {
              setMuted(player.volume || 0.9);
              player.setVolume(0);
            }
          }}
        />
        <input
          type="range"
          className="volume"
          min={0}
          max={1}
          step={0.02}
          value={player.volume}
          aria-label="Volume"
          onChange={(e) => {
            setMuted(null);
            player.setVolume(Number(e.target.value));
          }}
        />
      </span>
      <IconButton size="sm" label="Close the player" icon={LuX} onClick={player.stop} />
    </section>
  );
}
