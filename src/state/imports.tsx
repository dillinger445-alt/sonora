/**
 * Importing music: native pickers, drag and drop, typed paths (browser
 * development), folder rescans, and feedback when indexing finishes.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { LuFolderInput } from 'react-icons/lu';
import type { CoreEvents, ImportReport } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { plural } from '../lib/format.ts';
import { parsePathList } from '../lib/paths.ts';
import { navigate } from '../lib/router.ts';
import { useCore, useCoreEvent } from './core.tsx';
import { useToast } from './toasts.tsx';

export interface ImportActions {
  addFiles: () => void;
  addFolders: () => void;
  addPaths: (paths: string[]) => Promise<boolean>;
  rescanFolders: (ids?: number[]) => void;
  /** True while files are dragged over the window. */
  dragging: boolean;
}

const ImportContext = createContext<ImportActions | null>(null);

export function useImports(): ImportActions {
  const v = useContext(ImportContext);
  if (!v) throw new Error('useImports outside ImportProvider');
  return v;
}


function describeReport(r: ImportReport): string {
  const parts: string[] = [];
  if (r.added) parts.push(`${plural(r.added, 'new track')}`);
  if (r.changed) parts.push(`${r.changed} changed`);
  if (r.relinked) parts.push(`${r.relinked} moved or renamed`);
  if (r.restored) parts.push(`${r.restored} found again`);
  if (r.markedMissing) parts.push(`${r.markedMissing} missing`);
  if (r.unchanged) parts.push(`${r.unchanged} unchanged`);
  if (r.skippedRemoved) parts.push(`${r.skippedRemoved} you removed left out`);
  if (r.notAudio) parts.push(`${plural(r.notAudio, 'file')} that ${r.notAudio === 1 ? 'is' : 'are'} not audio left out`);
  return parts.length ? parts.join(', ') : 'No audio files found';
}

export function ImportProvider({ children }: { children: ReactNode }) {
  const { bridge, client } = useCore();
  const toast = useToast();
  const [dragging, setDragging] = useState(false);
  const [pathDialog, setPathDialog] = useState<null | 'files' | 'folders' | 'drop'>(null);
  const [text, setText] = useState('');
  const [invalid, setInvalid] = useState<string[]>([]);

  const addPaths = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return false;
      try {
        await client.importPaths(paths);
        toast({
          tone: 'info',
          title: paths.length === 1 ? 'Looking for audio files' : `Looking for audio files in ${paths.length} locations`,
          body: 'New and changed files are analyzed automatically.',
          action: { label: 'View queue', run: () => navigate({ view: 'queue' }) },
        });
        return true;
      } catch (e) {
        toast({ tone: 'error', title: 'Could not import', body: e instanceof Error ? e.message : String(e) });
        return false;
      }
    },
    [client, toast],
  );

  const addFiles = useCallback(() => {
    if (!bridge.native) {
      setPathDialog('files');
      return;
    }
    bridge.pickFiles().then(
      (paths) => {
        if (paths && paths.length) void addPaths(paths);
      },
      (e: unknown) => toast({ tone: 'error', title: 'Could not open the file picker', body: e instanceof Error ? e.message : String(e) }),
    );
  }, [bridge, addPaths, toast]);

  const addFolders = useCallback(() => {
    if (!bridge.native) {
      setPathDialog('folders');
      return;
    }
    bridge.pickFolders().then(
      (paths) => {
        if (paths && paths.length) void addPaths(paths);
      },
      (e: unknown) => toast({ tone: 'error', title: 'Could not open the folder picker', body: e instanceof Error ? e.message : String(e) }),
    );
  }, [bridge, addPaths, toast]);

  const rescanFolders = useCallback(
    (ids?: number[]) => {
      client.rescanFolders(ids).then(
        () =>
          toast({
            tone: 'info',
            title: ids && ids.length === 1 ? 'Checking the folder for changes' : 'Checking library folders for changes',
            body: 'Only new and changed files are analyzed.',
          }),
        (e: unknown) => toast({ tone: 'error', title: 'Could not rescan', body: e instanceof Error ? e.message : String(e) }),
      );
    },
    [client, toast],
  );

  // Drag and drop onto the window.
  useEffect(() => {
    let off: (() => void) | null = null;
    let disposed = false;
    bridge
      .onFileDrop({
        onHover: setDragging,
        onDrop: (paths) => {
          setDragging(false);
          if (paths.length) void addPaths(paths);
          else setPathDialog('drop');
        },
      })
      .then((u) => {
        if (disposed) u();
        else off = u;
      })
      .catch((e: unknown) => console.warn('drag and drop unavailable', e));
    return () => {
      disposed = true;
      off?.();
    };
  }, [bridge, addPaths]);

  useCoreEvent(['library.indexed'], (_name, payload) => {
    const p = payload as ImportReport | { error: string };
    if ('error' in p) {
      toast({ tone: 'error', title: 'Indexing failed', body: p.error });
      return;
    }
    if (p.cancelled) {
      toast({ tone: 'warning', title: 'Indexing cancelled', body: describeReport(p) });
      return;
    }
    // Downloads from the Soulseek folder have their own message (below).
    if (p.source) return;
    const nothingElse = !p.added && !p.changed && !p.relinked && !p.restored && !p.unchanged && !p.queued && !p.errors.length;
    if (nothingElse && p.notAudio) {
      toast({
        tone: 'warning',
        title: p.notAudio === 1 ? 'That file is not audio' : `Those ${p.notAudio} files are not audio`,
        body: 'SONORA adds audio files: WAV, AIFF, FLAC, ALAC, MP3, AAC or M4A, Ogg, Opus and similar.',
      });
      return;
    }
    if (p.waiting && p.queued > 0) {
      toast({
        tone: 'info',
        title: `Indexed: ${plural(p.queued, 'file')} waiting`,
        body: `${describeReport(p)}. The cancelled scan was still stopping, so analysis did not start. Start it from the Import queue.`,
        action: { label: 'Import queue', run: () => navigate({ view: 'queue' }) },
        timeout: 0,
      });
      return;
    }
    toast({
      tone: p.errors.length ? 'warning' : 'success',
      title: p.queued > 0 ? `Indexed: ${plural(p.queued, 'file')} to analyze` : 'Indexing finished',
      body: p.errors.length ? `${describeReport(p)}. ${plural(p.errors.length, 'item')} could not be read, for example: ${p.errors[0]}` : describeReport(p),
    });
  });

  useCoreEvent(['sources.imported'], (_name, payload) => {
    const p = payload as CoreEvents['sources.imported'];
    if (p.added > 0 || p.failed > 0) {
      toast({
        tone: p.failed > 0 ? 'warning' : 'success',
        title: p.added > 0 ? `Added ${plural(p.added, 'download')} from Soulseek` : `${plural(p.failed, 'download')} could not be added`,
        body:
          p.added > 0 && p.failed > 0
            ? `${plural(p.failed, 'other file')} could not be read as audio. The rest are being analysed.`
            : p.added > 0
              ? 'SONORA is analysing them now.'
              : 'SONORA could not read them as audio. The files stay in the download folder.',
        action: { label: 'Show downloads', run: () => navigate({ view: 'soulseek', tab: 'downloads' }) },
      });
    }
    if (p.first) {
      toast({ tone: 'info', title: 'Files from Soulseek come from other people', body: 'SONORA checks how a file sounds, not whether you may have it.', timeout: 0 });
    }
  });

  const submit = async () => {
    const { paths, invalid: bad } = parsePathList(text);
    setInvalid(bad);
    if (bad.length || paths.length === 0) return;
    if (await addPaths(paths)) {
      setPathDialog(null);
      setText('');
    }
  };

  const value = useMemo<ImportActions>(() => ({ addFiles, addFolders, addPaths, rescanFolders, dragging }), [addFiles, addFolders, addPaths, rescanFolders, dragging]);

  return (
    <ImportContext.Provider value={value}>
      {children}
      {dragging && bridge.native ? (
        <div className="drop-overlay" aria-hidden>
          <div className="drop-card">
            <LuFolderInput className="drop-icon" />
            <div className="drop-title">Drop to add to the library</div>
            <div className="drop-sub">Folders are scanned for audio files, including subfolders.</div>
          </div>
        </div>
      ) : null}
      <Dialog
        open={pathDialog != null}
        title={pathDialog === 'folders' ? 'Add folders by path' : 'Add files or folders by path'}
        onClose={() => {
          setPathDialog(null);
          setInvalid([]);
        }}
        width={540}
        initialFocus="textarea"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPathDialog(null)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void submit()} disabled={!text.trim()}>
              Add to library
            </Button>
          </>
        }
      >
        <p className="dialog-text">
          {pathDialog === 'drop'
            ? 'Browsers do not reveal where dropped files live, so drag and drop needs the desktop app. Enter the full paths instead, one per line.'
            : 'This browser preview cannot open the system file picker. Enter full paths of files or folders on this computer, one per line. The desktop app uses the native picker and drag and drop.'}
        </p>
        <label className="field">
          <span className="field-label">Paths</span>
          <textarea
            className="textarea num-ish selectable"
            rows={5}
            value={text}
            placeholder={'/Users/me/Music/Techno\n/Volumes/USB/Sets/2024'}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                void submit();
              }
            }}
          />
        </label>
        {invalid.length ? (
          <p className="field-error" role="alert">
            Use full paths (starting with / or a drive letter). Not accepted: {invalid.slice(0, 3).join(', ')}
            {invalid.length > 3 ? '…' : ''}
          </p>
        ) : null}
      </Dialog>
    </ImportContext.Provider>
  );
}
