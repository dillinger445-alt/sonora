import { useEffect } from 'react';
import { LuServerOff } from 'react-icons/lu';
import type { Bridge } from './api/bridge.ts';
import { Button } from './components/Button.tsx';
import { PlayerBar } from './components/PlayerBar.tsx';
import { NAV_ORDER, Sidebar } from './components/Sidebar.tsx';
import { StatusBar } from './components/StatusBar.tsx';
import { TooltipLayer } from './components/TooltipLayer.tsx';
import { navigate, useRoute, type Route } from './lib/router.ts';
import { CoreProvider, useCore } from './state/core.tsx';
import { ImportProvider, useImports } from './state/imports.tsx';
import { JobsProvider } from './state/jobs.tsx';
import { PlayerProvider, usePlayer } from './state/player.tsx';
import { ToastProvider } from './state/toasts.tsx';
import { UpdatesProvider } from './state/updates.tsx';
import { AttentionView } from './views/Attention.tsx';
import { DuplicatesView } from './views/Duplicates.tsx';
import { FoldersView } from './views/Folders.tsx';
import { HistoryView } from './views/History.tsx';
import { LibraryView } from './views/Library.tsx';
import { OverviewView } from './views/Overview.tsx';
import { QueueView } from './views/Queue.tsx';
import { SettingsView } from './views/Settings.tsx';
import { TrackView } from './views/Track.tsx';
import { UpdateDialog } from './views/UpdateDialog.tsx';

export function App({ bridge }: { bridge: Bridge }) {
  return (
    <ToastProvider>
      <CoreProvider bridge={bridge}>
        <ImportProvider>
          <JobsProvider>
            <UpdatesProvider>
              <PlayerProvider>
                <Shell />
              </PlayerProvider>
            </UpdatesProvider>
          </JobsProvider>
        </ImportProvider>
      </CoreProvider>
    </ToastProvider>
  );
}

function renderView(route: Route) {
  switch (route.view) {
    case 'overview':
      return <OverviewView />;
    case 'library':
      return <LibraryView key={route.folderId ?? 'all'} folderId={route.folderId} />;
    case 'queue':
      return <QueueView />;
    case 'folders':
      return <FoldersView />;
    case 'attention':
      return <AttentionView flag={route.flag} />;
    case 'duplicates':
      return <DuplicatesView />;
    case 'history':
      return <HistoryView scanId={route.scanId} />;
    case 'settings':
      return <SettingsView />;
    case 'track':
      return <TrackView key={route.id} id={route.id} />;
  }
}

function Shell() {
  const route = useRoute();
  const { fatal, retry, bridge } = useCore();
  const imports = useImports();
  const player = usePlayer();

  // Global shortcuts: ⌘/Ctrl+1…7 views, ⌘/Ctrl+, settings, ⌘/Ctrl+O import.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod || e.altKey) return;
      const n = Number(e.key);
      if (Number.isInteger(n) && n >= 1 && n <= 7 && !e.shiftKey) {
        const r = NAV_ORDER[n - 1];
        if (r) {
          e.preventDefault();
          navigate(r);
        }
      } else if (e.key === ',') {
        e.preventDefault();
        navigate({ view: 'settings' });
      } else if (e.key.toLowerCase() === 'o') {
        e.preventDefault();
        if (e.shiftKey) imports.addFolders();
        else imports.addFiles();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [imports]);

  // P plays or pauses the loaded track, unless the focused element uses the
  // key itself (typing, or the track table playing its active row).
  const { track, status, pause, resume } = player;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || (e.key !== 'p' && e.key !== 'P')) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
      // Not while a dialog is open.
      if (document.querySelector('dialog[open]')) return;
      if (!track) return;
      e.preventDefault();
      if (status === 'playing' || status === 'loading') pause();
      else resume();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [track, status, pause, resume]);

  // Keep the document title informative (window title on Windows/Linux).
  useEffect(() => {
    const names: Record<string, string> = {
      overview: 'Overview',
      library: 'Music library',
      queue: 'Import queue',
      folders: 'Folders',
      attention: 'Needs attention',
      duplicates: 'Duplicates',
      history: 'Analysis history',
      settings: 'Settings',
      track: 'Track analysis',
    };
    document.title = `${names[route.view] ?? ''} – SONORA`;
  }, [route.view]);

  if (fatal) {
    return (
      <div className="fatal" role="alert">
        <LuServerOff className="fatal-icon" aria-hidden />
        <h1 className="fatal-title">The analysis core is not available</h1>
        <p className="fatal-body selectable">{fatal}</p>
        {bridge.kind === 'http' ? (
          <p className="fatal-body faint">
            In browser development the interface talks to the local dev bridge. Start both with <code>npm run dev:web</code>.
          </p>
        ) : null}
        <Button variant="primary" onClick={retry}>
          Try again
        </Button>
      </div>
    );
  }

  const inset = bridge.kind === 'tauri' && bridge.platform === 'macos';
  return (
    <div className={`app ${inset ? 'has-titlebar-inset' : ''}`}>
      {/* A button, not an anchor: the hash is used for routing. */}
      <button type="button" className="skip-link" onClick={() => document.getElementById('main')?.focus()}>
        Skip to content
      </button>
      <Sidebar active={route.view} />
      <main className="workspace" id="main" tabIndex={-1}>
        {renderView(route)}
      </main>
      <PlayerBar />
      <StatusBar />
      <UpdateDialog />
      <TooltipLayer />
    </div>
  );
}
