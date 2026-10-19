/**
 * Application state shared by every view: the API client, library counts
 * and settings (`useCore`), and the live scan status (`useScan`, kept apart
 * because it changes several times a second while a scan runs). Views fetch
 * their own data and subscribe to core events through `useCoreEvent`.
 */

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { Bridge } from '../api/bridge.ts';
import { Client } from '../api/client.ts';
import type { AppInfo, ImportReport, Overview, ScanStatus, Settings } from '../api/types.ts';
import { Emitter, throttle } from './emitter.ts';

export interface CoreContextValue {
  bridge: Bridge;
  client: Client;
  events: Emitter;
  info: AppInfo | null;
  settings: Settings | null;
  lastIndexReport: ImportReport | null;
  overview: Overview | null;
  duplicateGroups: number | null;
  /** Set when the core cannot be reached at all. */
  fatal: string | null;
  refreshOverview: () => void;
  refreshInfo: () => Promise<void>;
  saveSettings: (values: Partial<Settings>) => Promise<Settings>;
  retry: () => void;
}

export interface ScanState {
  scan: ScanStatus | null;
  /** Snapshot sent with the most recent `scan.finished` event. */
  lastFinished: ScanStatus | null;
}

const CoreContext = createContext<CoreContextValue | null>(null);
const ScanContext = createContext<ScanState>({ scan: null, lastFinished: null });
const BusyContext = createContext(false);

export function useCore(): CoreContextValue {
  const v = useContext(CoreContext);
  if (!v) throw new Error('useCore outside CoreProvider');
  return v;
}

/** Live scan status; re-renders on every progress update, so use it only where progress is shown. */
export function useScan(): ScanState {
  return useContext(ScanContext);
}

/** Whether indexing or analysis is running; re-renders only when that changes. */
export function useScanBusy(): boolean {
  return useContext(BusyContext);
}

/** Subscribes to core events (names matched exactly or by `prefix.*`). */
export function useCoreEvent(names: string[], handler: (name: string, payload: unknown) => void): void {
  const { events } = useCore();
  const ref = useRef(handler);
  useLayoutEffect(() => {
    ref.current = handler;
  });
  const key = names.join('|');
  useEffect(() => {
    const list = key.split('|');
    return events.on((name, payload) => {
      if (list.some((n) => n === name || (n.endsWith('.*') && name.startsWith(n.slice(0, -1))))) {
        ref.current(name, payload);
      }
    });
  }, [events, key]);
}

export function CoreProvider({ bridge, children }: { bridge: Bridge; children: ReactNode }) {
  const client = useMemo(() => new Client(bridge), [bridge]);
  const events = useMemo(() => new Emitter(), []);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [scan, setScan] = useState<ScanStatus | null>(null);
  const [lastFinished, setLastFinished] = useState<ScanStatus | null>(null);
  const [lastIndexReport, setLastIndexReport] = useState<ImportReport | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [duplicateGroups, setDuplicateGroups] = useState<number | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  const loadOverview = useCallback(() => {
    client.overview().then(setOverview, (e: unknown) => console.warn('overview failed', e));
  }, [client]);

  const overviewThrottle = useMemo(() => throttle(loadOverview, 1500), [loadOverview]);

  const loadDuplicates = useCallback(() => {
    client.duplicates().then(
      (d) => setDuplicateGroups(d.groups.length),
      (e: unknown) => console.warn('duplicates failed', e),
    );
  }, [client]);

  const duplicatesThrottle = useMemo(() => throttle(loadDuplicates, 5000), [loadDuplicates]);

  const refreshInfo = useCallback(async () => {
    setInfo(await client.info());
  }, [client]);

  // Initial load, and again after "Try again".
  useEffect(() => {
    let cancelled = false;
    setFatal(null);
    Promise.all([client.info(), client.settings(), client.scanStatus(), client.overview()])
      .then(([i, s, st, ov]) => {
        if (cancelled) return;
        setInfo(i);
        setSettings(s);
        setScan(st);
        setOverview(ov);
        loadDuplicates();
      })
      .catch((e: unknown) => {
        if (!cancelled) setFatal(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [client, attempt, loadDuplicates]);

  // Core events → shared state, then fan out to views.
  useEffect(() => {
    let unsubscribe: (() => void) | null = null;
    let disposed = false;
    client
      .subscribe((name, payload) => {
        switch (name) {
          case 'scan.started':
          case 'scan.progress':
            setScan((payload as { scan: ScanStatus }).scan);
            break;
          case 'scan.finished': {
            const snap = (payload as { scan: ScanStatus }).scan;
            setLastFinished(snap);
            client.scanStatus().then(setScan, () => undefined);
            overviewThrottle.run();
            duplicatesThrottle.run();
            break;
          }
          case 'scan.item':
            overviewThrottle.run();
            break;
          case 'library.indexed':
            if (payload && typeof payload === 'object' && !('error' in payload)) setLastIndexReport(payload as ImportReport);
            client.scanStatus().then(setScan, () => undefined);
            break;
          case 'library.changed':
            overviewThrottle.run();
            duplicatesThrottle.run();
            break;
          case 'duplicates.updated':
            loadDuplicates();
            break;
          default:
            break;
        }
        events.emit(name, payload);
      })
      .then((u) => {
        if (disposed) u();
        else unsubscribe = u;
      })
      .catch((e: unknown) => console.error('event subscription failed', e));
    return () => {
      disposed = true;
      unsubscribe?.();
      overviewThrottle.cancel();
      duplicatesThrottle.cancel();
    };
  }, [client, events, overviewThrottle, duplicatesThrottle, loadDuplicates]);

  // Status polling while busy, in case an event was missed (e.g. the
  // window was asleep). Events remain the primary source.
  const busy = scan != null && scan.state !== 'idle';
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(() => {
      client.scanStatus().then(setScan, () => undefined);
    }, 5000);
    return () => clearInterval(t);
  }, [busy, client]);

  const saveSettings = useCallback(
    async (values: Partial<Settings>) => {
      const s = await client.updateSettings(values);
      setSettings(s);
      return s;
    },
    [client],
  );

  const value = useMemo<CoreContextValue>(
    () => ({
      bridge,
      client,
      events,
      info,
      settings,
      lastIndexReport,
      overview,
      duplicateGroups,
      fatal,
      refreshOverview: loadOverview,
      refreshInfo,
      saveSettings,
      retry: () => setAttempt((a) => a + 1),
    }),
    [bridge, client, events, info, settings, lastIndexReport, overview, duplicateGroups, fatal, loadOverview, refreshInfo, saveSettings],
  );
  const scanState = useMemo<ScanState>(() => ({ scan, lastFinished }), [scan, lastFinished]);

  return (
    <CoreContext.Provider value={value}>
      <BusyContext.Provider value={busy}>
        <ScanContext.Provider value={scanState}>{children}</ScanContext.Provider>
      </BusyContext.Provider>
    </CoreContext.Provider>
  );
}
