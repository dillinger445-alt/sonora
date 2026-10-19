import { useMemo } from 'react';
import { LuFolderPlus, LuLibrary, LuPlus, LuX } from 'react-icons/lu';
import { Button } from '../components/Button.tsx';
import { EmptyState, ViewHeader } from '../components/Layout.tsx';
import { formatCount, formatSpan } from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useImports } from '../state/imports.tsx';
import { useResource } from '../state/useResource.ts';
import { LibraryBrowser } from './LibraryBrowser.tsx';

export function LibraryView({ folderId }: { folderId?: number }) {
  const { overview, client } = useCore();
  const imports = useImports();
  const folders = useResource(() => client.folders(), [], { enabled: folderId != null });
  const folder = folderId != null ? folders.data?.find((f) => f.id === folderId) : undefined;
  const baseFilter = useMemo(() => (folderId != null ? { folderId } : undefined), [folderId]);
  const t = overview?.tracks;

  return (
    <div className="view view-fill">
      <ViewHeader
        title={folder ? folder.name : 'Music library'}
        subtitle={
          folderId != null ? (
            <span className="selectable">{folder?.path ?? ''}</span>
          ) : t && t.total > 0 ? (
            `${formatCount(t.total)} tracks${overview?.totalDuration ? `, ${formatSpan(overview.totalDuration)} of audio` : ''}. Double-click a track for its full analysis.`
          ) : undefined
        }
        actions={
          folderId != null ? (
            <Button icon={LuX} onClick={() => navigate({ view: 'library' })}>
              Show all tracks
            </Button>
          ) : undefined
        }
      />
      <LibraryBrowser
        prefsKey="library"
        label={folder ? `Tracks in ${folder.name}` : 'Music library'}
        baseFilter={baseFilter}
        defaultSort={{ key: 'title', desc: false }}
        emptyBase={
          folderId != null ? (
            <EmptyState icon={LuLibrary} title="No tracks in this folder">
              Rescan the folder from the Folders view to pick up new files.
            </EmptyState>
          ) : (
            <EmptyState
              icon={LuLibrary}
              title="Your library is empty"
              actions={
                <>
                  <Button variant="primary" icon={LuFolderPlus} onClick={imports.addFolders}>
                    Add folder…
                  </Button>
                  <Button icon={LuPlus} onClick={imports.addFiles}>
                    Add files…
                  </Button>
                </>
              }
            >
              Add a music folder and SONORA analyzes every track in the background. Files stay where they are and nothing is uploaded.
            </EmptyState>
          )
        }
      />
    </div>
  );
}
