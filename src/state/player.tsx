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
import { CoreError } from '../api/bridge.ts';
import type { TrackRow } from '../api/types.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { useCore, useCoreEvent } from './core.tsx';
import { throttle } from './emitter.ts';

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
  // What the listener last asked for: a pause while the file is still being
  // prepared must not be undone when it is ready.
  const wantPlay = useRef(false);

  /** The current attempt is still waiting for the core to prepare its file. */
  const preparing = useCallback((): Attempt | null => {
    const a = attempt.current;
    return a != null && a.id === seq.current && a.src == null && !a.failed ? a : null;
  }, []);
  const stateRef = useRef(state);
  useLayoutEffect(() => {
    stateRef.current = state;
  });

  // ----------------------------------------------------------- position
  const listeners = useRef(new Set<() => void>());
  const time = useRef(0);
  const publish = useCallback(() => {
    // The element still holds the previous file while the next is prepared.
    if (preparing()) return;
    const t = audio.currentTime;
    if (t === time.current) return;
    time.current = t;
    for (const l of [...listeners.current]) l();
  }, [audio, preparing]);
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

  const notify = useCallback((t: number) => {
    time.current = t;
    for (const l of [...listeners.current]) l();
  }, []);

  /** The current attempt failed: a format the web view cannot read is decoded once in the core. */
  const fail = useCallback((a: Attempt, formatError: boolean, message: string) => {
    if (a.failed || a.id !== seq.current) return;
    a.failed = true;
    if (formatError && !a.decoded) {
      void loadRef.current(a.track, time.current, true);
      return;
    }
    setState((s) => ({ ...s, status: 'error', error: message }));
  }, []);

  const load = useCallback(
    async (track: PlayableTrack, at: number, decode: boolean) => {
      const id = ++seq.current;
      const a: Attempt = { id, track, at, decoded: decode, src: null, failed: false };
      attempt.current = a;
      wantPlay.current = true;
      // The previous track stops now, not when the next one is ready.
      audio.pause();
      pendingSeek.current = null;
      notify(at);
      setState((s) => ({ ...s, track, status: 'loading', error: null, duration: s.track?.id === track.id ? s.duration : (track.duration ?? null) }));
      let url: string;
      let transcoded: boolean;
      try {
        const prepared = await client.preparePlayback(track.id, playable, decode);
        if (id !== seq.current) return;
        url = bridge.mediaUrl(prepared);
        transcoded = prepared.transcoded;
      } catch (e) {
        if (id === seq.current) {
          a.failed = true;
          setState((s) => ({ ...s, status: 'error', error: e instanceof Error ? e.message : String(e) }));
        }
        return;
      }
      // Seeks made while the file was prepared moved `a.at`.
      pendingSeek.current = a.at > 0 ? a.at : null;
      notify(a.at);
      audio.src = url;
      a.src = audio.src;
      setState((s) => ({ ...s, transcoded }));
      if (!wantPlay.current) {
        // Paused while preparing: ready to play from the chosen position.
        setState((s) => ({ ...s, status: 'paused' }));
        return;
      }
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
    [audio, bridge, client, playable, fail, notify],
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
  /** The file being prepared will start playing (from `at`, if given) once it is ready. */
  const playWhenReady = useCallback(
    (a: Attempt, at?: number) => {
      wantPlay.current = true;
      if (at != null) {
        a.at = Math.max(0, at);
        notify(a.at);
      }
      setState((s) => (s.track ? { ...s, status: 'loading' } : s));
    },
    [notify],
  );

  const play = useCallback(
    (track: PlayableTrack, at = 0) => {
      const s = stateRef.current;
      const a = preparing();
      if (a && a.track.id === track.id) {
        playWhenReady(a, at);
        return;
      }
      if (s.track?.id === track.id && s.status !== 'error' && audio.getAttribute('src')) {
        wantPlay.current = true;
        audio.currentTime = Math.max(0, at);
        void audio.play().catch(() => undefined);
        return;
      }
      void load(track, at, false);
    },
    [audio, load, preparing, playWhenReady],
  );

  const resume = useCallback(() => {
    const s = stateRef.current;
    if (!s.track) return;
    const a = preparing();
    if (a) {
      playWhenReady(a);
      return;
    }
    if (s.status === 'error' || !audio.getAttribute('src')) {
      void load(s.track, time.current, attempt.current?.decoded ?? false);
      return;
    }
    wantPlay.current = true;
    if (audio.ended) audio.currentTime = 0;
    void audio.play().catch(() => undefined);
  }, [audio, load, preparing, playWhenReady]);

  const pause = useCallback(() => {
    wantPlay.current = false;
    audio.pause();
    // While the file is prepared (or buffering) no pause event changes the
    // status, so it changes here.
    setState((s) => (s.track && s.status === 'loading' ? { ...s, status: 'paused' } : s));
  }, [audio]);

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
      const a = preparing();
      // While preparing, the element still holds the previous file.
      const d = !a && Number.isFinite(audio.duration) ? audio.duration : (stateRef.current.duration ?? seconds);
      const t = Math.max(0, Math.min(d, seconds));
      if (a) a.at = t;
      else if (audio.readyState === 0) pendingSeek.current = t;
      else audio.currentTime = t;
      notify(t);
    },
    [audio, preparing, notify],
  );

  const stop = useCallback(() => {
    seq.current += 1;
    attempt.current = null;
    wantPlay.current = false;
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

  // Keep the title, tempo and key shown in the player current when the
  // track is edited or analyzed again.
  const refreshTrack = useMemo(
    () =>
      throttle(() => {
        const id = stateRef.current.track?.id;
        if (id == null) return;
        client.track(id, { visuals: false }).then(
          ({ track: t }) =>
            setState((s) =>
              s.track?.id === id ? { ...s, track: { id: t.id, displayTitle: t.displayTitle, artist: t.artist, duration: t.duration, djBpm: t.djBpm, djKey: t.djKey } } : s,
            ),
          (e: unknown) => {
            // The track left the library (removed, or its file moved to the Trash): stop playing it.
            if (e instanceof CoreError && e.code === 'not_found' && stateRef.current.track?.id === id) stop();
          },
        );
      }, 1000),
    [client, stop],
  );
  useEffect(() => () => refreshTrack.cancel(), [refreshTrack]);
  useCoreEvent(['library.changed', 'scan.finished'], () => {
    if (stateRef.current.track) refreshTrack.run();
  });

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
