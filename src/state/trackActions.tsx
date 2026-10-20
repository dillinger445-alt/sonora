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
import { usePlaylists } from './playlists.tsx';
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
  /** Asks, then moves the files to the system's Trash and removes their tracks; true when any went. */
  trash: (ids: number[], what?: string) => Promise<boolean>;
  /** "Trash" or, on Windows, "Recycle Bin". */
  trashName: string;
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
  const playlists = usePlaylists();
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

  /** " It is also taken out of “Warm-up”." when the tracks are in playlists. */
  const inPlaylists = useCallback(
    async (ids: number[]) => {
      try {
        const { ids: lists } = await client.playlistsContainingAny(ids);
        if (lists.length === 0) return '';
        const names = lists.map((id) => playlists.byId.get(id)?.name).filter((n): n is string => !!n);
        const where = names.length === lists.length && names.length <= 2 ? names.map((n) => `“${n}”`).join(' and ') : plural(lists.length, 'playlist');
        return ` ${ids.length === 1 ? 'It is' : 'They are'} also taken out of ${where}.`;
      } catch {
        return '';
      }
    },
    [client, playlists.byId],
  );

  const remove = useCallback(
    async (ids: number[], what?: string) => {
      if (ids.length === 0) return false;
      const one = ids.length === 1;
      const name = what ?? plural(ids.length, 'track');
      const lists = await inPlaylists(ids);
      const { confirmed } = await confirm({
        title: one ? 'Remove from library?' : `Remove ${plural(ids.length, 'track')} from the library?`,
        body: (
          <>
            <p>
              {name} will be removed from SONORA's library, with {one ? 'its' : 'their'} analysis results and any tempo, key and genre corrections.{lists}
            </p>
            <p className="faint">
              The audio {one ? 'file stays where it is' : 'files stay where they are'}, and rescans of {one ? 'its' : 'their'} folder leave {one ? 'it' : 'them'} out. Adding the{' '}
              {one ? 'file' : 'files'} again, or Bring back in Folders, returns {one ? 'it' : 'them'} to the library.
            </p>
          </>
        ),
        confirmLabel: 'Remove from library',
      });
      if (!confirmed) return false;
      try {
        const r = await client.remove(ids);
        if (r.busy > 0) {
          toast({
            tone: 'warning',
            title: `Removed ${plural(r.removed, 'track')} from the library`,
            body: `${plural(r.busy, 'track')} being analyzed right now ${r.busy === 1 ? 'stays' : 'stay'}: remove ${r.busy === 1 ? 'it' : 'them'} once the analysis is done.`,
            timeout: 9000,
          });
        } else {
          toast({ tone: 'success', title: `Removed ${plural(r.removed, 'track')} from the library` });
        }
        return r.removed > 0;
      } catch (e) {
        toast({ tone: 'error', title: 'Could not remove', body: e instanceof Error ? e.message : String(e) });
        return false;
      }
    },
    [client, confirm, toast, inPlaylists],
  );

  const trashName = bridge.platform === 'windows' ? 'Recycle Bin' : 'Trash';
  const trash = useCallback(
    async (ids: number[], what?: string) => {
      if (ids.length === 0) return false;
      const one = ids.length === 1;
      const lists = await inPlaylists(ids);
      const { confirmed } = await confirm({
        title: one ? `Move the file to the ${trashName}?` : `Move ${plural(ids.length, 'file')} to the ${trashName}?`,
        body: (
          <>
            <p>
              {what ?? (one ? 'The audio file' : `The ${plural(ids.length, 'audio file')}`)} will be moved from {one ? 'its' : 'their'} folder to the {trashName}, and{' '}
              {one ? 'its track' : 'their tracks'} removed from SONORA's library, with the analysis results.{lists}
            </p>
            <p className="faint">
              You can put {one ? 'it' : 'them'} back from the {trashName} until you empty it.
            </p>
          </>
        ),
        confirmLabel: `Move to ${trashName}`,
      });
      if (!confirmed) return false;
      try {
        const r = await client.trash(ids);
        const gone = r.removed - r.trashed;
        const missing = gone > 0 ? `${plural(gone, 'track')} whose file${gone === 1 ? ' was' : 's were'} already gone left the library too.` : undefined;
        const busy = r.busy > 0 ? `${plural(r.busy, 'track')} being analyzed right now ${r.busy === 1 ? 'was' : 'were'} left alone.` : undefined;
        if (r.failed.length === 0) {
          toast({
            tone: busy ? 'warning' : 'success',
            title: r.trashed ? `Moved ${plural(r.trashed, 'file')} to the ${trashName}` : `Removed ${plural(r.removed, 'track')} from the library`,
            body: [r.trashed ? missing : undefined, busy].filter(Boolean).join(' ') || undefined,
          });
        } else {
          const first = r.failed[0];
          toast({
            tone: 'warning',
            title: r.trashed ? `Moved ${formatCount(r.trashed)} of ${plural(ids.length, 'file')} to the ${trashName}` : `Could not move ${one ? 'the file' : 'the files'} to the ${trashName}`,
            body: `${first ? `${first.title}: ${first.error}` : ''}${r.failed.length > 1 ? ` (and ${plural(r.failed.length - 1, 'more')})` : ''}. ${r.failed.length === 1 ? 'It stays' : 'They stay'} in the library.`,
            timeout: 12000,
          });
        }
        return r.removed > 0;
      } catch (e) {
        toast({ tone: 'error', title: `Could not move to the ${trashName}`, body: e instanceof Error ? e.message : String(e) });
        return false;
      }
    },
    [client, confirm, toast, trashName, inPlaylists],
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
            action: r.path
              ? {
                  label: revealLabel(bridge.platform),
                  run: () => void bridge.reveal(r.path as string).catch((e: unknown) => toast({ tone: 'error', title: 'Could not show the file', body: e instanceof Error ? e.message : String(e) })),
                }
              : undefined,
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
                SONORA sends the artist and title of {ids.length === 1 ? 'this track' : 'each track'} to Discogs with your token: up to three searches a track, one a second
                {ids.length > 5 ? ` (at most about ${formatSpan(ids.length * 3.3)})` : ''}. Nothing else leaves this computer.
              </p>
              <p className="faint">Tracks that already have a genre keep it unless you choose to replace it.</p>
            </>
          ),
          confirmLabel: 'Look up genres',
          tone: 'primary',
          option: { label: 'Replace genres the tracks already have' },
        });
        if (!confirmed) return;
        // Running before the call returns: its finish event may come first.
        jobs.started('discogs');
        try {
          await client.discogsLookup(ids, option);
        } catch (e) {
          jobs.resync();
          throw e;
        }
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

  return { reanalyze, remove, trash, trashName, exportCsv, reveal, revealText: revealLabel(bridge.platform), editDj, exportDj, lookupGenres, dialogs };
}
