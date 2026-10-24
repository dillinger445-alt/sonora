/**
 * Background jobs besides analysis: DJ exports, sending playlists to a USB
 * drive, Discogs genre lookups, and YouTube playlists downloaded from
 * Soulseek.
 * Both run in the core; this follows their events so the status bar can
 * show progress from any view, and reports the outcome when nobody is
 * watching it in a dialog.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { EVENTS_RECONNECTED } from '../api/bridge.ts';
import type { CoreEvents, DiscogsReport, ExportProgress, ExportReport, PlaylistRun, UsbProgress, UsbReport } from '../api/types.ts';
import { formatCount, plural } from '../lib/format.ts';
import { revealLabel } from '../lib/platform.ts';
import { navigate } from '../lib/router.ts';
import { useCore, useCoreEvent } from './core.tsx';
import { useToast } from './toasts.tsx';

export type DiscogsProgress = CoreEvents['discogs.progress'];

export interface Jobs {
  exportRunning: boolean;
  exportProgress: ExportProgress | null;
  exportReport: ExportReport | null;
  discogsRunning: boolean;
  discogsProgress: DiscogsProgress | null;
  usbRunning: boolean;
  usbProgress: UsbProgress | null;
  /** The YouTube playlist being searched and downloaded, while one is. */
  playlistRun: PlaylistRun | null;
  /** While the playlist page shows the run, its outcome is not announced again. Returns the release. */
  watchPlaylist: () => () => void;
  /** While a dialog shows the export, its outcome is not announced again. Returns the release. */
  watchExport: () => () => void;
  /** The same for sending to a drive. */
  watchUsb: () => () => void;
  /** Marks a job as started, before asking the core: a short job may finish before the answer arrives. */
  started: (job: 'export' | 'discogs' | 'usb') => void;
  /** Asks the core which jobs run (after a start call failed). */
  resync: () => void;
  cancelExport: () => void;
  cancelDiscogs: () => void;
  cancelUsb: () => void;
  cancelPlaylist: () => void;
}

/** One line describing a finished playlist run. */
export function describePlaylist(r: PlaylistRun): { tone: 'success' | 'warning' | 'error' | 'info'; title: string; body?: string } {
  const c = r.counts;
  const name = r.title ?? 'YouTube playlist';
  if (r.state === 'cancelled') return { tone: 'info', title: `Stopped: ${name}`, body: `${plural(c.done, 'track')} downloaded before it stopped.` };
  if (r.state === 'failed') return { tone: 'error', title: `${name} stopped`, body: r.error ?? undefined };
  const parts = [c.notFound ? `${plural(c.notFound, 'track')} not found` : null, c.failed ? `${plural(c.failed, 'track')} failed` : null].filter(Boolean);
  if (c.done === 0) return { tone: 'warning', title: `Nothing downloaded from ${name}`, body: parts.join(', ') || undefined };
  return { tone: parts.length ? 'warning' : 'success', title: `${name}: ${plural(c.done, 'track')} downloaded and checked`, body: parts.join(', ') || undefined };
}

/** One line describing a finished send to a drive. */
export function describeUsb(r: UsbReport): { tone: 'success' | 'warning' | 'error' | 'info'; title: string; body?: string } {
  if (r.error) return { tone: 'error', title: 'Sending stopped', body: r.error };
  const parts = [r.written ? `${plural(r.written, 'file')} written` : null, r.renamed ? `${formatCount(r.renamed)} renamed` : null, r.removed ? `${formatCount(r.removed)} removed` : null, r.kept ? `${formatCount(r.kept)} unchanged` : null].filter(Boolean);
  const body = parts.join(', ') || undefined;
  if (r.cancelled) return { tone: 'info', title: 'Sending stopped', body };
  const failed = r.failed ?? [];
  if (failed.length) return { tone: 'warning', title: `Sent, but ${plural(failed.length, 'track')} failed`, body: failed[0]?.error };
  return { tone: 'success', title: `Sent ${plural(r.playlists, 'playlist')} to the drive`, body };
}

const JobsContext = createContext<Jobs | null>(null);

export function useJobs(): Jobs {
  const v = useContext(JobsContext);
  if (!v) throw new Error('useJobs outside JobsProvider');
  return v;
}

/** One line describing a finished Discogs lookup. */
export function describeDiscogs(r: DiscogsReport): { tone: 'success' | 'warning' | 'error' | 'info'; title: string; body?: string } {
  if (r.error && !r.found) return { tone: 'error', title: 'The Discogs lookup stopped', body: r.error };
  const parts: string[] = [];
  if (r.notFound) parts.push(`${plural(r.notFound, 'track')} not found`);
  if (r.skipped) parts.push(`${plural(r.skipped, 'track')} skipped (already had a genre, or no artist and title)`);
  if (r.failed) parts.push(`${plural(r.failed, 'request')} failed`);
  const body = [parts.join(', '), r.error ?? ''].filter(Boolean).join('. ') || undefined;
  if (r.cancelled) return { tone: 'info', title: `Lookup stopped after finding ${plural(r.found, 'genre')}`, body };
  return { tone: r.found > 0 ? 'success' : 'warning', title: r.found > 0 ? `Found genres for ${plural(r.found, 'track')} on Discogs` : 'No genres found on Discogs', body };
}

export function JobsProvider({ children }: { children: ReactNode }) {
  const { client, bridge } = useCore();
  const toast = useToast();
  const [exportRunning, setExportRunning] = useState(false);
  const [exportProgress, setExportProgress] = useState<ExportProgress | null>(null);
  const [exportReport, setExportReport] = useState<ExportReport | null>(null);
  const [discogsRunning, setDiscogsRunning] = useState(false);
  const [discogsProgress, setDiscogsProgress] = useState<DiscogsProgress | null>(null);
  const [usbRunning, setUsbRunning] = useState(false);
  const [usbProgress, setUsbProgress] = useState<UsbProgress | null>(null);
  const [playlistRun, setPlaylistRun] = useState<PlaylistRun | null>(null);
  const watchers = useRef(0);
  const usbWatchers = useRef(0);
  const playlistWatchers = useRef(0);

  const resync = useCallback(() => {
    client.exportStatus().then(
      (s) => {
        setExportRunning(s.running);
        if (!s.running) setExportProgress(null);
      },
      () => undefined,
    );
    client.discogsStatus().then(
      (s) => {
        setDiscogsRunning(s.running);
        if (!s.running) setDiscogsProgress(null);
      },
      () => undefined,
    );
    client.usbStatus().then(
      (s) => {
        setUsbRunning(s.running);
        if (!s.running) setUsbProgress(null);
      },
      () => undefined,
    );
    client.playlistList().then(
      (r) => setPlaylistRun(r.runs.find((x) => x.state === 'running' || x.state === 'paused') ?? null),
      () => undefined,
    );
  }, [client]);

  // A job may already run (the interface was reloaded during it).
  useEffect(resync, [resync]);

  // A `*.finished` missed while the event stream was down would leave a job
  // showing as running (B96).
  useCoreEvent([EVENTS_RECONNECTED], resync);

  useCoreEvent(['djexport.progress', 'djexport.finished', 'discogs.progress', 'discogs.finished', 'usb.progress', 'usb.finished', 'playlist.changed', 'playlist.finished'], (name, payload) => {
    switch (name) {
      case 'playlist.changed': {
        const r = payload as CoreEvents['playlist.changed'];
        if (r.state === 'fetching') break;
        const run = r as PlaylistRun;
        setPlaylistRun((cur) => (run.state === 'running' || run.state === 'paused' ? run : cur && cur.id === run.id ? null : cur));
        break;
      }
      case 'playlist.finished': {
        const r = payload as PlaylistRun;
        setPlaylistRun((cur) => (cur && cur.id === r.id ? null : cur));
        if (playlistWatchers.current > 0) break;
        const d = describePlaylist(r);
        toast({ ...d, action: { label: 'Show', run: () => navigate({ view: 'soulseek', tab: 'playlist' }) }, timeout: d.tone === 'error' ? 0 : 9000 });
        break;
      }
      case 'usb.progress':
        setUsbRunning(true);
        setUsbProgress(payload as UsbProgress);
        break;
      case 'usb.finished': {
        setUsbRunning(false);
        setUsbProgress(null);
        if (usbWatchers.current > 0) break;
        const r = payload as UsbReport;
        const d = describeUsb(r);
        const folder = r.folder;
        toast({
          ...d,
          action:
            folder && !r.error
              ? {
                  label: revealLabel(bridge.platform),
                  run: () => void bridge.reveal(folder).catch((e: unknown) => toast({ tone: 'error', title: 'Could not show the files', body: e instanceof Error ? e.message : String(e) })),
                }
              : undefined,
          timeout: d.tone === 'error' ? 0 : 8000,
        });
        break;
      }
      case 'djexport.progress':
        setExportRunning(true);
        setExportProgress(payload as ExportProgress);
        break;
      case 'djexport.finished': {
        const r = payload as ExportReport;
        setExportRunning(false);
        setExportProgress(null);
        setExportReport(r);
        if (watchers.current > 0) break;
        // A report of an export that stopped on an internal error has only `error` and `folder`.
        const written = r.written ?? [];
        const failed = r.failed ?? [];
        const first = r.rekordboxXml ?? written[0]?.path;
        const reveal = first
          ? {
              label: revealLabel(bridge.platform),
              run: () => void bridge.reveal(first).catch((e: unknown) => toast({ tone: 'error', title: 'Could not show the files', body: e instanceof Error ? e.message : String(e) })),
            }
          : undefined;
        if (r.error) toast({ tone: 'error', title: 'The export stopped', body: r.error });
        else if (r.cancelled) toast({ tone: 'info', title: `Export stopped after ${plural(written.length, 'track')}`, action: reveal });
        else if (failed.length) toast({ tone: 'warning', title: `Exported ${plural(written.length, 'track')}, ${failed.length} failed`, body: failed[0]?.error, action: reveal });
        else toast({ tone: 'success', title: `Exported ${plural(written.length, 'track')} for DJ software`, body: r.folder, action: reveal });
        break;
      }
      case 'discogs.progress':
        setDiscogsRunning(true);
        setDiscogsProgress(payload as DiscogsProgress);
        break;
      case 'discogs.finished': {
        setDiscogsRunning(false);
        setDiscogsProgress(null);
        const d = describeDiscogs(payload as DiscogsReport);
        const settings = d.tone === 'error' ? { label: 'Settings', run: () => navigate({ view: 'settings' }) } : undefined;
        toast({ ...d, action: settings, timeout: d.tone === 'error' ? 0 : 7000 });
        break;
      }
      default:
        break;
    }
  });

  const watchExport = useCallback(() => {
    watchers.current += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      watchers.current -= 1;
    };
  }, []);

  const watchPlaylist = useCallback(() => {
    playlistWatchers.current += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      playlistWatchers.current -= 1;
    };
  }, []);

  const watchUsb = useCallback(() => {
    usbWatchers.current += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      usbWatchers.current -= 1;
    };
  }, []);

  const started = useCallback((job: 'export' | 'discogs' | 'usb') => {
    if (job === 'export') {
      setExportRunning(true);
      setExportProgress(null);
    } else if (job === 'usb') {
      setUsbRunning(true);
      setUsbProgress(null);
    } else {
      setDiscogsRunning(true);
      setDiscogsProgress(null);
    }
  }, []);

  // A stop that fails is said: the job keeps running, and a silent button looks broken.
  const cancelFailed = useCallback(
    (what: string) => (e: unknown) => {
      toast({ tone: 'error', title: `Could not stop ${what}`, body: e instanceof Error ? e.message : String(e) });
      resync();
    },
    [toast, resync],
  );
  const cancelExport = useCallback(() => {
    client.exportCancel().catch(cancelFailed('the export'));
  }, [client, cancelFailed]);
  const cancelDiscogs = useCallback(() => {
    client.discogsCancel().catch(cancelFailed('the Discogs lookup'));
  }, [client, cancelFailed]);
  const cancelUsb = useCallback(() => {
    client.usbCancel().catch(cancelFailed('sending to the drive'));
  }, [client, cancelFailed]);
  const cancelPlaylist = useCallback(() => {
    setPlaylistRun((r) => {
      if (r) client.playlistCancel(r.id).catch(cancelFailed('the playlist'));
      return r;
    });
  }, [client, cancelFailed]);

  const value = useMemo<Jobs>(
    () => ({
      exportRunning,
      exportProgress,
      exportReport,
      discogsRunning,
      discogsProgress,
      usbRunning,
      usbProgress,
      playlistRun,
      watchPlaylist,
      watchExport,
      watchUsb,
      started,
      resync,
      cancelExport,
      cancelDiscogs,
      cancelUsb,
      cancelPlaylist,
    }),
    [exportRunning, exportProgress, exportReport, discogsRunning, discogsProgress, usbRunning, usbProgress, playlistRun, watchPlaylist, watchExport, watchUsb, started, resync, cancelExport, cancelDiscogs, cancelUsb, cancelPlaylist],
  );
  return <JobsContext.Provider value={value}>{children}</JobsContext.Provider>;
}
