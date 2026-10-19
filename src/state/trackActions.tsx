/** Actions on tracks shared by the library, inspector, analysis and duplicates views. */

import { useCallback, useState, type ReactNode } from 'react';
import type { QueueResult } from '../api/client.ts';
import type { LibraryFilter } from '../api/types.ts';
import { useConfirm } from '../components/Dialog.tsx';
import { formatCount, formatSpan, plural } from '../lib/format.ts';
import { revealLabel } from '../lib/platform.ts';
import { navigate } from '../lib/router.ts';
import { DjEditDialog, type DjEditTarget } from '../views/DjEditDialog.tsx';
import { DjExportDialog, type ExportTarget } from '../views/DjExportDialog.tsx';
import { useCore } from './core.tsx';
import { useJobs } from './jobs.tsx';
import { useToast, type ToastInput } from './toasts.tsx';

export { revealLabel };

/** Toast describing what a request to analyze tracks did. */
export function describeQueued(r: QueueResult): ToastInput {
  const view = { label: 'View queue', run: () => navigate({ view: 'queue' }) };
  const already = r.alreadyQueued ?? 0;
  if (r.queued > 0) {
    return {
      tone: 'info',
      title: `Queued ${plural(r.queued, 'track')} for analysis`,
      body: already > 0 ? `${plural(already, 'other track')} ${already === 1 ? 'was' : 'were'} already waiting.` : undefined,
      action: view,
    };
  }
  if (already > 0) {
    return { tone: 'info', title: already === 1 ? 'Already waiting in the queue' : `All ${formatCount(already)} tracks are already waiting in the queue`, action: view };
  }
  if (r.missing) {
    return { tone: 'warning', title: 'Nothing to analyze', body: r.missing === 1 ? 'The file is missing from disk.' : 'The files are missing from disk.' };
  }
  return { tone: 'warning', title: 'Nothing to analyze' };
}

export interface TrackActions {
  reanalyze: (ids: number[]) => Promise<void>;
  remove: (ids: number[], what?: string) => Promise<boolean>;
  exportCsv: (target: { ids: number[] } | { filter: LibraryFilter }, count: number) => Promise<void>;
  reveal: (path: string) => Promise<void>;
  revealText: string;
  /** Opens the dialog that corrects tempo, key and genre. */
  editDj: (target: DjEditTarget) => void;
  /** Opens the DJ export dialog for these tracks. */
  exportDj: (ids: number[]) => void;
  /** Asks, then looks up genres on Discogs (artist and title only). */
  lookupGenres: (ids: number[]) => Promise<void>;
  /** Render this once in the view (confirmation and DJ dialogs). */
  dialogs: ReactNode;
}

function downloadText(name: string, text: string) {
  const blob = new Blob([text], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

function exportName(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `sonora-export-${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}.csv`;
}

export function useTrackActions(): TrackActions {
  const { client, bridge } = useCore();
  const toast = useToast();
  const jobs = useJobs();
  const [confirm, confirmDialog] = useConfirm();
  const [editTarget, setEditTarget] = useState<DjEditTarget | null>(null);
  const [exportTarget, setExportTarget] = useState<ExportTarget | null>(null);

  const reanalyze = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return;
      try {
        toast(describeQueued(await client.rescan(ids)));
      } catch (e) {
        toast({ tone: 'error', title: 'Could not queue the analysis', body: e instanceof Error ? e.message : String(e) });
      }
    },
    [client, toast],
  );

  const remove = useCallback(
    async (ids: number[], what?: string) => {
      if (ids.length === 0) return false;
      const name = what ?? plural(ids.length, 'track');
      const { confirmed } = await confirm({
        title: ids.length === 1 ? 'Remove from library?' : `Remove ${plural(ids.length, 'track')} from the library?`,
        body: (
          <>
            <p>
              {name} will be removed from SONORA's library together with {ids.length === 1 ? 'its' : 'their'} analysis results.
            </p>
            <p className="faint">The audio files stay where they are; nothing on disk is deleted. Importing the files again brings them back.</p>
          </>
        ),
        confirmLabel: 'Remove from library',
      });
      if (!confirmed) return false;
      try {
        const r = await client.remove(ids);
        toast({ tone: 'success', title: `Removed ${plural(r.removed, 'track')} from the library` });
        return true;
      } catch (e) {
        toast({ tone: 'error', title: 'Could not remove', body: e instanceof Error ? e.message : String(e) });
        return false;
      }
    },
    [client, confirm, toast],
  );

  const exportCsv = useCallback(
    async (target: { ids: number[] } | { filter: LibraryFilter }, count: number) => {
      try {
        if (bridge.native) {
          const path = await bridge.pickSavePath(exportName());
          if (!path) return;
          const r = await client.exportCsv(target, path);
          toast({
            tone: 'success',
            title: `Exported ${plural(r.rows, 'track')}`,
            body: r.path,
            action: r.path ? { label: revealLabel(bridge.platform), run: () => void bridge.reveal(r.path as string) } : undefined,
          });
        } else {
          const r = await client.exportCsv(target);
          downloadText(exportName(), r.csv ?? '');
          toast({ tone: 'success', title: `Exported ${plural(r.rows, 'track')}`, body: 'Saved by the browser to your downloads.' });
        }
      } catch (e) {
        toast({ tone: 'error', title: `Could not export ${plural(count, 'track')}`, body: e instanceof Error ? e.message : String(e) });
      }
    },
    [bridge, client, toast],
  );

  const reveal = useCallback(
    async (path: string) => {
      try {
        await bridge.reveal(path);
      } catch (e) {
        toast({ tone: 'error', title: 'Could not show the file', body: e instanceof Error ? e.message : String(e) });
      }
    },
    [bridge, toast],
  );

  const editDj = useCallback((target: DjEditTarget) => {
    if (target.ids.length) setEditTarget(target);
  }, []);
  const exportDj = useCallback((ids: number[]) => {
    if (ids.length) setExportTarget({ ids });
  }, []);

  const lookupGenres = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return;
      try {
        const st = await client.discogsStatus();
        if (!st.configured) {
          const { confirmed } = await confirm({
            title: 'Connect Discogs first',
            body: (
              <>
                <p>Genre lookups use your free Discogs account. Add a personal access token in Settings → Genres; it stays on this computer.</p>
                <p className="faint">Without Discogs, genres come from the files' tags, from folder names, or from what you type.</p>
              </>
            ),
            confirmLabel: 'Open Settings',
            tone: 'primary',
          });
          if (confirmed) navigate({ view: 'settings' });
          return;
        }
        if (st.running) {
          toast({ tone: 'warning', title: 'A Discogs lookup is already running' });
          return;
        }
        const { confirmed, option } = await confirm({
          title: ids.length === 1 ? 'Look up the genre on Discogs?' : `Look up genres for ${plural(ids.length, 'track')} on Discogs?`,
          body: (
            <>
              <p>
                SONORA sends the artist and title of {ids.length === 1 ? 'this track' : 'each track'} to Discogs, one request a second
                {ids.length > 5 ? ` (about ${formatSpan(ids.length * 1.1)})` : ''}. Nothing else leaves this computer.
              </p>
              <p className="faint">Tracks that already have a genre keep it unless you choose to replace it.</p>
            </>
          ),
          confirmLabel: 'Look up genres',
          tone: 'primary',
          option: { label: 'Replace genres the tracks already have' },
        });
        if (!confirmed) return;
        await client.discogsLookup(ids, option);
        jobs.started('discogs');
      } catch (e) {
        toast({ tone: 'error', title: 'Could not start the Discogs lookup', body: e instanceof Error ? e.message : String(e) });
      }
    },
    [client, confirm, toast, jobs],
  );

  const dialogs = (
    <>
      {confirmDialog}
      <DjEditDialog target={editTarget} onClose={() => setEditTarget(null)} />
      <DjExportDialog target={exportTarget} onClose={() => setExportTarget(null)} />
    </>
  );

  return { reanalyze, remove, exportCsv, reveal, revealText: revealLabel(bridge.platform), editDj, exportDj, lookupGenres, dialogs };
}
