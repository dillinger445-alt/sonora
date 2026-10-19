/**
 * Listening inside the app: one audio element for the whole window.
 *
 * The core hands over the file to play (the original when this web view can
 * decode its format, otherwise a decoded copy in SONORA's cache), and the
 * bridge turns that path into a URL the audio element may load. Nothing
 * leaves the computer.
 *
 * Position updates are published through `usePlaybackTime`, which only the
 * small components that draw a playhead subscribe to, so the rest of the
 * interface does not re-render while a track plays.
 */

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { TrackRow } from '../api/types.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { useCore } from './core.tsx';

/** What the player needs to know about a track. */
export type PlayableTrack = Pick<TrackRow, 'id' | 'displayTitle' | 'artist' | 'duration' | 'djBpm' | 'djKey'>;

export type PlayerStatus = 'loading' | 'playing' | 'paused' | 'error';

export interface PlayerState {
  track: PlayableTrack | null;
  status: PlayerStatus;
  error: string | null;
  /** The file is played from a decoded copy (the web view cannot read the original's format). */
  transcoded: boolean;
  /** Duration reported by the audio element (falls back to the library's). */
  duration: number | null;
}

export interface Player extends PlayerState {
  volume: number;
  /** Plays `track` from `at` seconds (from the start when it is a new track). */
  play: (track: PlayableTrack, at?: number) => void;
  /** Pauses the current track if it is `track`, otherwise plays `track`. */
  toggle: (track: PlayableTrack) => void;
  pause: () => void;
  resume: () => void;
  seek: (seconds: number) => void;
  /** Stops and closes the player. */
  stop: () => void;
  setVolume: (v: number) => void;
  /** Whether `id` is the loaded track, and playing (or about to). */
  isPlaying: (id: number) => boolean;
}

interface TimeSource {
  subscribe: (cb: () => void) => () => void;
  get: () => number;
}

const PlayerContext = createContext<Player | null>(null);
const TimeContext = createContext<TimeSource>({ subscribe: () => () => undefined, get: () => 0 });

export function usePlayer(): Player {
  const v = useContext(PlayerContext);
  if (!v) throw new Error('usePlayer outside PlayerProvider');
  return v;
}

/** The playback position in seconds, updated every animation frame while playing. */
export function usePlaybackTime(): number {
  const src = useContext(TimeContext);
  return useSyncExternalStore(src.subscribe, src.get, src.get);
}

/** Formats the web view can decode, by the names `playback.prepare` uses. */
function playableKinds(audio: HTMLAudioElement): string[] {
  const probes: [string, string][] = [
    ['mp3', 'audio/mpeg'],
    ['aac', 'audio/mp4; codecs="mp4a.40.2"'],
    ['alac', 'audio/mp4; codecs="alac"'],
    ['flac', 'audio/flac'],
    ['vorbis', 'audio/ogg; codecs="vorbis"'],
    ['opus', 'audio/ogg; codecs="opus"'],
    ['wav', 'audio/wav; codecs="1"'],
    ['wavfloat', 'audio/wav; codecs="3"'],
    ['aiff', 'audio/aiff'],
  ];
  // "maybe" is the usual answer for containers; a wrong guess falls back to
  // a decoded copy when the element reports a decode error.
  return probes.filter(([, mime]) => audio.canPlayType(mime) !== '').map(([k]) => k);
}

const isVolume = (v: unknown): v is number => typeof v === 'number' && v >= 0 && v <= 1;

interface Attempt {
  id: number;
  track: PlayableTrack;
  at: number;
  /** Playing SONORA's decoded copy rather than the original. */
  decoded: boolean;
  /** The URL given to the audio element, once it is set. */
  src: string | null;
  failed: boolean;
}

export function PlayerProvider({ children }: { children: ReactNode }) {
  const { client, bridge } = useCore();
  const [audio] = useState(() => {
    const a = new Audio();
    a.preload = 'auto';
    return a;
  });
  const [state, setState] = useState<PlayerState>({ track: null, status: 'paused', error: null, transcoded: false, duration: null });
  const [volume, setVolumeState] = useState(() => readPref('player.volume', 0.9, isVolume));
  const playable = useMemo(() => playableKinds(audio), [audio]);

  // Which request is current: a newer `play` makes older answers irrelevant.
  const seq = useRef(0);
  const attempt = useRef<Attempt | null>(null);
  const pendingSeek = useRef<number | null>(null);
  const stateRef = useRef(state);
  useLayoutEffect(() => {
    stateRef.current = state;
  });

  // ----------------------------------------------------------- position
  const listeners = useRef(new Set<() => void>());
  const time = useRef(0);
  const publish = useCallback(() => {
    const t = audio.currentTime;
    if (t === time.current) return;
    time.current = t;
    for (const l of [...listeners.current]) l();
  }, [audio]);
  const timeSource = useMemo<TimeSource>(
    () => ({
      subscribe: (cb) => {
        listeners.current.add(cb);
        return () => {
          listeners.current.delete(cb);
        };
      },
      get: () => time.current,
    }),
    [],
  );
  useEffect(() => {
    if (state.status !== 'playing') return;
    let raf = 0;
    const tick = () => {
      publish();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [state.status, publish]);

  // ------------------------------------------------------------ loading
  const loadRef = useRef<(track: PlayableTrack, at: number, decode: boolean) => Promise<void>>(async () => undefined);

  /** The current attempt failed: a format the web view cannot read is decoded once in the core. */
  const fail = useCallback((a: Attempt, formatError: boolean, message: string) => {
    if (a.failed || a.id !== seq.current) return;
    a.failed = true;
    if (formatError && !a.decoded) {
      void loadRef.current(a.track, a.at, true);
      return;
    }
    setState((s) => ({ ...s, status: 'error', error: message }));
  }, []);

  const load = useCallback(
    async (track: PlayableTrack, at: number, decode: boolean) => {
      const id = ++seq.current;
      const a: Attempt = { id, track, at, decoded: decode, src: null, failed: false };
      attempt.current = a;
      setState((s) => ({ ...s, track, status: 'loading', error: null, duration: s.track?.id === track.id ? s.duration : (track.duration ?? null) }));
      let prepared;
      try {
        prepared = await client.preparePlayback(track.id, playable, decode);
      } catch (e) {
        if (id === seq.current) setState((s) => ({ ...s, status: 'error', error: e instanceof Error ? e.message : String(e) }));
        return;
      }
      if (id !== seq.current) return;
      pendingSeek.current = at > 0 ? at : null;
      time.current = at;
      for (const l of [...listeners.current]) l();
      audio.src = bridge.mediaUrl(prepared.path);
      a.src = audio.src;
      setState((s) => ({ ...s, transcoded: prepared.transcoded }));
      try {
        await audio.play();
      } catch (e) {
        if (id !== seq.current) return;
        const name = e instanceof DOMException ? e.name : '';
        // A newer source replaced this one.
        if (name === 'AbortError') return;
        // Autoplay was refused (no click to start it): wait for the play button.
        if (name === 'NotAllowedError') {
          setState((s) => ({ ...s, status: 'paused' }));
          return;
        }
        fail(a, name === 'NotSupportedError', e instanceof Error ? e.message : String(e));
      }
    },
    [audio, bridge, client, playable, fail],
  );
  useLayoutEffect(() => {
    loadRef.current = load;
  });

  useEffect(() => {
    const onPlaying = () => setState((s) => (s.track ? { ...s, status: 'playing', error: null } : s));
    const onPause = () => {
      publish();
      setState((s) => (s.track && s.status !== 'loading' && s.status !== 'error' ? { ...s, status: 'paused' } : s));
    };
    const onWaiting = () => setState((s) => (s.track && s.status === 'playing' ? { ...s, status: 'loading' } : s));
    const onMeta = () => {
      if (pendingSeek.current != null) {
        audio.currentTime = Math.min(pendingSeek.current, Math.max(0, audio.duration - 0.05));
        pendingSeek.current = null;
      }
      const d = Number.isFinite(audio.duration) ? audio.duration : null;
      setState((s) => ({ ...s, duration: d ?? s.duration }));
    };
    const onEnded = () => {
      publish();
      setState((s) => ({ ...s, status: 'paused' }));
    };
    const onError = () => {
      const a = attempt.current;
      // Only errors of the source the current attempt set count.
      if (!a || a.src == null || audio.src !== a.src) return;
      const code = audio.error?.code;
      fail(a, code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED || code === MediaError.MEDIA_ERR_DECODE, audio.error?.message || 'The audio could not be played.');
    };
    audio.addEventListener('playing', onPlaying);
    audio.addEventListener('pause', onPause);
    audio.addEventListener('waiting', onWaiting);
    audio.addEventListener('loadedmetadata', onMeta);
    audio.addEventListener('ended', onEnded);
    audio.addEventListener('error', onError);
    audio.addEventListener('seeked', publish);
    audio.addEventListener('timeupdate', publish);
    return () => {
      audio.removeEventListener('playing', onPlaying);
      audio.removeEventListener('pause', onPause);
      audio.removeEventListener('waiting', onWaiting);
      audio.removeEventListener('loadedmetadata', onMeta);
      audio.removeEventListener('ended', onEnded);
      audio.removeEventListener('error', onError);
      audio.removeEventListener('seeked', publish);
      audio.removeEventListener('timeupdate', publish);
    };
  }, [audio, fail, publish]);

  useEffect(() => {
    audio.volume = volume;
  }, [audio, volume]);

  // ------------------------------------------------------------ actions
  const play = useCallback(
    (track: PlayableTrack, at = 0) => {
      const s = stateRef.current;
      if (s.track?.id === track.id && s.status !== 'error' && audio.getAttribute('src')) {
        audio.currentTime = Math.max(0, at);
        void audio.play().catch(() => undefined);
        return;
      }
      void load(track, at, false);
    },
    [audio, load],
  );

  const resume = useCallback(() => {
    const s = stateRef.current;
    if (!s.track) return;
    if (s.status === 'error' || !audio.getAttribute('src')) {
      void load(s.track, time.current, attempt.current?.decoded ?? false);
      return;
    }
    if (audio.ended) audio.currentTime = 0;
    void audio.play().catch(() => undefined);
  }, [audio, load]);

  const pause = useCallback(() => audio.pause(), [audio]);

  const toggle = useCallback(
    (track: PlayableTrack) => {
      const s = stateRef.current;
      if (s.track?.id !== track.id) play(track);
      else if (s.status === 'playing' || s.status === 'loading') pause();
      else resume();
    },
    [play, pause, resume],
  );

  const seek = useCallback(
    (seconds: number) => {
      if (!stateRef.current.track) return;
      const d = Number.isFinite(audio.duration) ? audio.duration : (stateRef.current.duration ?? seconds);
      const t = Math.max(0, Math.min(d, seconds));
      if (audio.readyState === 0) pendingSeek.current = t;
      else audio.currentTime = t;
      time.current = t;
      for (const l of [...listeners.current]) l();
    },
    [audio],
  );

  const stop = useCallback(() => {
    seq.current += 1;
    attempt.current = null;
    audio.pause();
    audio.removeAttribute('src');
    audio.load();
    time.current = 0;
    setState({ track: null, status: 'paused', error: null, transcoded: false, duration: null });
  }, [audio]);

  const setVolume = useCallback((v: number) => {
    const c = Math.max(0, Math.min(1, v));
    setVolumeState(c);
    writePref('player.volume', c);
  }, []);

  // Media keys and the system's now-playing controls.
  useEffect(() => {
    const ms = typeof navigator !== 'undefined' ? navigator.mediaSession : undefined;
    if (!ms) return;
    const t = state.track;
    try {
      ms.metadata = t ? new MediaMetadata({ title: t.displayTitle, artist: t.artist ?? '', album: 'SONORA' }) : null;
      ms.playbackState = !t ? 'none' : state.status === 'playing' ? 'playing' : 'paused';
    } catch {
      // Not supported by this web view.
    }
  }, [state.track, state.status]);
  useEffect(() => {
    const ms = typeof navigator !== 'undefined' ? navigator.mediaSession : undefined;
    if (!ms) return;
    const handlers: [MediaSessionAction, MediaSessionActionHandler][] = [
      ['play', () => resume()],
      ['pause', () => pause()],
      ['stop', () => stop()],
      ['seekbackward', (d) => seek(time.current - (d.seekOffset ?? 10))],
      ['seekforward', (d) => seek(time.current + (d.seekOffset ?? 10))],
      ['seekto', (d) => seek(d.seekTime ?? time.current)],
    ];
    for (const [a, h] of handlers) {
      try {
        ms.setActionHandler(a, h);
      } catch {
        // Unsupported action.
      }
    }
    return () => {
      for (const [a] of handlers) {
        try {
          ms.setActionHandler(a, null);
        } catch {
          // Unsupported action.
        }
      }
    };
  }, [resume, pause, stop, seek]);

  // Stop on unmount (window closing, or the interface reloading).
  useEffect(() => () => audio.pause(), [audio]);

  const isPlaying = useCallback((id: number) => state.track?.id === id && (state.status === 'playing' || state.status === 'loading'), [state.track, state.status]);

  const value = useMemo<Player>(
    () => ({ ...state, volume, play, toggle, pause, resume, seek, stop, setVolume, isPlaying }),
    [state, volume, play, toggle, pause, resume, seek, stop, setVolume, isPlaying],
  );

  return (
    <PlayerContext.Provider value={value}>
      <TimeContext.Provider value={timeSource}>{children}</TimeContext.Provider>
    </PlayerContext.Provider>
  );
}
