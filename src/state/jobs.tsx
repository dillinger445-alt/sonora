/**
 * Background jobs besides analysis: DJ exports, sending playlists to a USB
 * drive, and Discogs genre lookups.
 * Both run in the core; this follows their events so the status bar can
 * show progress from any view, and reports the outcome when nobody is
 * watching it in a dialog.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { CoreEvents, DiscogsReport, ExportProgress, ExportReport, UsbProgress, UsbReport } from '../api/types.ts';
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
  const watchers = useRef(0);
  const usbWatchers = useRef(0);

  const resync = useCallback(() => {
    client.exportStatus().then(
      (s) => setExportRunning(s.running),
      () => undefined,
    );
    client.discogsStatus().then(
      (s) => setDiscogsRunning(s.running),
      () => undefined,
    );
    client.usbStatus().then(
      (s) => setUsbRunning(s.running),
      () => undefined,
    );
  }, [client]);

  // A job may already run (the interface was reloaded during it).
  useEffect(resync, [resync]);

  useCoreEvent(['djexport.progress', 'djexport.finished', 'discogs.progress', 'discogs.finished', 'usb.progress', 'usb.finished'], (name, payload) => {
    switch (name) {
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
        toast({ ...d, action: folder && !r.error ? { label: revealLabel(bridge.platform), run: () => void bridge.reveal(folder).catch(() => undefined) } : undefined, timeout: d.tone === 'error' ? 0 : 8000 });
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

  const value = useMemo<Jobs>(
    () => ({
      exportRunning,
      exportProgress,
      exportReport,
      discogsRunning,
      discogsProgress,
      usbRunning,
      usbProgress,
      watchExport,
      watchUsb,
      started,
      resync,
      cancelExport,
      cancelDiscogs,
      cancelUsb,
    }),
    [exportRunning, exportProgress, exportReport, discogsRunning, discogsProgress, usbRunning, usbProgress, watchExport, watchUsb, started, resync, cancelExport, cancelDiscogs, cancelUsb],
  );
  return <JobsContext.Provider value={value}>{children}</JobsContext.Provider>;
}
