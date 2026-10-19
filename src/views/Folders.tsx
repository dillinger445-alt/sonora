import { LuFolder, LuFolderPlus, LuFolderSearch, LuRefreshCw, LuTrash2, LuTriangleAlert } from 'react-icons/lu';
import type { Folder } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { useConfirm } from '../components/Dialog.tsx';
import { EmptyState, ErrorState, ViewHeader } from '../components/Layout.tsx';
import { ScoreCell } from '../components/Marks.tsx';
import { formatBytes, formatCount, formatRelative, plural } from '../lib/format.ts';
import { focusMain } from '../lib/focus.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore, useScanBusy } from '../state/core.tsx';
import { useImports } from '../state/imports.tsx';
import { useToast } from '../state/toasts.tsx';
import { revealLabel } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';

export function FoldersView() {
  const { client, bridge } = useCore();
  const busy = useScanBusy();
  const imports = useImports();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const folders = useResource(() => client.folders(), [], { events: ['library.changed', 'library.indexed', 'scan.finished', 'scan.item'], throttleMs: 1500 });

  const remove = async (f: Folder) => {
    const { confirmed, option } = await confirm({
      title: `Stop watching “${f.name}”?`,
      body: (
        <>
          <p>The folder will no longer be part of the library and will not be rescanned.</p>
          <p className="faint">Nothing on disk is deleted.</p>
        </>
      ),
      confirmLabel: 'Remove folder',
      option: f.tracks > 0 ? { label: `Also remove its ${plural(f.tracks, 'track')} and their results from the library`, defaultValue: false } : undefined,
    });
    if (!confirmed) return;
    try {
      const r = await client.removeFolder(f.id, option);
      toast({ tone: 'success', title: `Removed “${f.name}”`, body: option ? `${plural(r.removedTracks, 'track')} removed from the library.` : 'Its tracks stay in the library.' });
      focusMain();
      folders.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not remove the folder', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const list = folders.data ?? [];
  return (
    <div className="view view-scroll">
      <ViewHeader
        title="Folders"
        subtitle="Library folders are scanned recursively. Rescanning picks up new, changed, moved and deleted files without re-analyzing unchanged ones."
        actions={
          <>
            <Button icon={LuRefreshCw} onClick={() => imports.rescanFolders()} disabled={list.length === 0 || busy}>
              Rescan all
            </Button>
            <Button variant="primary" icon={LuFolderPlus} onClick={imports.addFolders}>
              Add folder…
            </Button>
          </>
        }
      />
      {folders.error ? <ErrorState title="Could not load folders" error={folders.error} onRetry={folders.reload} /> : null}
      {!folders.loading && list.length === 0 && !folders.error ? (
        <EmptyState
          icon={LuFolder}
          title="No library folders yet"
          actions={
            <Button variant="primary" icon={LuFolderPlus} onClick={imports.addFolders}>
              Add folder…
            </Button>
          }
        >
          Folders you add are remembered, so you can rescan them later for new and changed files. Files added one by one do not need a folder.
        </EmptyState>
      ) : null}
      {list.length > 0 ? (
        <table className="list-table folders-table">
          <thead>
            <tr>
              <th>Folder</th>
              <th className="is-end">Tracks</th>
              <th className="is-end">Analyzed</th>
              <th className="is-end">Problems</th>
              <th className="is-end" data-tip="Average overall score">
                Avg.
              </th>
              <th className="is-end">Size</th>
              <th>Last scan</th>
              <th>
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {list.map((f) => (
              <tr key={f.id} className={f.available ? '' : 'is-unavailable'}>
                <td className="cell-title-2">
                  <a className="folder-name row-link" href={formatRoute({ view: 'library', folderId: f.id })} data-tip="Show this folder's tracks">
                    {f.available ? <LuFolder aria-hidden className="inline-icon" /> : <LuTriangleAlert aria-hidden className="inline-icon t-amber" />}
                    {f.name}
                  </a>
                  <span className="ellipsis faint selectable" title={f.path}>
                    {f.available ? f.path : `${f.path} (not available: disconnected drive?)`}
                  </span>
                </td>
                <td className="is-end num">{formatCount(f.tracks)}</td>
                <td className="is-end num muted">{formatCount(f.analyzed)}</td>
                <td className="is-end num">
                  {f.failed + f.missing > 0 ? (
                    <span className="t-amber" data-tip={`${formatCount(f.failed)} failed, ${formatCount(f.missing)} missing`}>
                      {formatCount(f.failed + f.missing)}
                    </span>
                  ) : (
                    <span className="faint">0</span>
                  )}
                </td>
                <td className="is-end">
                  <ScoreCell score={f.avgOverall} />
                </td>
                <td className="is-end num muted">{formatBytes(f.bytes)}</td>
                <td className="faint">{f.lastScanAt ? formatRelative(f.lastScanAt) : 'Never'}</td>
                <td className="cell-actions">
                  <IconButton size="sm" label="Rescan this folder" icon={LuRefreshCw} disabled={!f.available || busy} onClick={() => imports.rescanFolders([f.id])} />
                  <IconButton size="sm" label={revealLabel(bridge.platform)} icon={LuFolderSearch} disabled={!f.available} onClick={() => void bridge.reveal(f.path).catch((e: unknown) => toast({ tone: 'error', title: 'Could not open the folder', body: String(e) }))} />
                  <IconButton size="sm" label="Remove folder from library" icon={LuTrash2} tone="danger" onClick={() => void remove(f)} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
      {dialogs}
    </div>
  );
}
