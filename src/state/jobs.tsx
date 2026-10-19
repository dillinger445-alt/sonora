/**
 * Background jobs besides analysis: DJ exports and Discogs genre lookups.
 * Both run in the core; this follows their events so the status bar can
 * show progress from any view, and reports the outcome when nobody is
 * watching it in a dialog.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { CoreEvents, DiscogsReport, ExportProgress, ExportReport } from '../api/types.ts';
import { plural } from '../lib/format.ts';
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
  /** While a dialog shows the export, its outcome is not announced again. Returns the release. */
  watchExport: () => () => void;
  /** Marks a job as started, before asking the core: a short job may finish before the answer arrives. */
  started: (job: 'export' | 'discogs') => void;
  /** Asks the core which jobs run (after a start call failed). */
  resync: () => void;
  cancelExport: () => void;
  cancelDiscogs: () => void;
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
  const watchers = useRef(0);

  const resync = useCallback(() => {
    client.exportStatus().then(
      (s) => setExportRunning(s.running),
      () => undefined,
    );
    client.discogsStatus().then(
      (s) => setDiscogsRunning(s.running),
      () => undefined,
    );
  }, [client]);

  // A job may already run (the interface was reloaded during it).
  useEffect(resync, [resync]);

  useCoreEvent(['djexport.progress', 'djexport.finished', 'discogs.progress', 'discogs.finished'], (name, payload) => {
    switch (name) {
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
        const reveal = first ? { label: revealLabel(bridge.platform), run: () => void bridge.reveal(first) } : undefined;
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

  const started = useCallback((job: 'export' | 'discogs') => {
    if (job === 'export') {
      setExportRunning(true);
      setExportProgress(null);
    } else {
      setDiscogsRunning(true);
      setDiscogsProgress(null);
    }
  }, []);

  const cancelExport = useCallback(() => {
    client.exportCancel().catch(() => undefined);
  }, [client]);
  const cancelDiscogs = useCallback(() => {
    client.discogsCancel().catch(() => undefined);
  }, [client]);

  const value = useMemo<Jobs>(
    () => ({ exportRunning, exportProgress, exportReport, discogsRunning, discogsProgress, watchExport, started, resync, cancelExport, cancelDiscogs }),
    [exportRunning, exportProgress, exportReport, discogsRunning, discogsProgress, watchExport, started, resync, cancelExport, cancelDiscogs],
  );
  return <JobsContext.Provider value={value}>{children}</JobsContext.Provider>;
}
