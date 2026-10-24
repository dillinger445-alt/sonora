import type { ComponentType } from 'react';
import { LuCopy, LuFolder, LuHistory, LuInbox, LuLayoutDashboard, LuLibrary, LuPlus, LuRadioTower, LuSettings, LuTriangleAlert } from 'react-icons/lu';
import { Button } from './Button.tsx';
import { Menu } from './Menu.tsx';
import { formatCount } from '../lib/format.ts';
import { formatRoute, type Route, type View } from '../lib/router.ts';
import { useCore, useScan, useScanBusy } from '../state/core.tsx';
import { useImports } from '../state/imports.tsx';
import { PlaylistTree } from './PlaylistTree.tsx';

type IconComponent = ComponentType<{ className?: string; 'aria-hidden'?: boolean }>;

interface NavItem {
  view: View;
  route: Route;
  label: string;
  icon: IconComponent;
  shortcut: string;
}

const GROUPS: NavItem[][] = [
  [
    { view: 'overview', route: { view: 'overview' }, label: 'Overview', icon: LuLayoutDashboard, shortcut: '1' },
    { view: 'library', route: { view: 'library' }, label: 'Music library', icon: LuLibrary, shortcut: '2' },
    { view: 'queue', route: { view: 'queue' }, label: 'Import queue', icon: LuInbox, shortcut: '3' },
    { view: 'folders', route: { view: 'folders' }, label: 'Folders', icon: LuFolder, shortcut: '4' },
  ],
  [
    { view: 'attention', route: { view: 'attention' }, label: 'Needs attention', icon: LuTriangleAlert, shortcut: '5' },
    { view: 'duplicates', route: { view: 'duplicates' }, label: 'Duplicates', icon: LuCopy, shortcut: '6' },
    { view: 'history', route: { view: 'history' }, label: 'Analysis history', icon: LuHistory, shortcut: '7' },
  ],
  [{ view: 'soulseek', route: { view: 'soulseek' }, label: 'Soulseek', icon: LuRadioTower, shortcut: '8' }],
];

export const NAV_ORDER: Route[] = [...GROUPS.flat().map((i) => i.route), { view: 'settings' }];

function Wordmark() {
  // The app icon's mark at its smallest: a meter whose scale is a sound wave
  // (drawn by src-tauri/icons/generate.py, "tiny").
  return (
    <div className="wordmark" data-tauri-drag-region>
      <svg className="wordmark-glyph" viewBox="8 17 84 66" aria-hidden>
        <g stroke="currentColor" strokeWidth="7.6" strokeLinecap="round">
          <line x1="19.6" y1="52.6" x2="12.6" y2="48.2" />
          <line x1="33.7" y1="42.2" x2="27.5" y2="31" />
          <line x1="50" y1="40" x2="50" y2="23.2" />
          <line x1="66.3" y1="42.2" x2="72.5" y2="31" />
          <line x1="80.4" y1="52.6" x2="87.4" y2="48.2" />
          <line x1="50" y1="71.6" x2="59.8" y2="47.4" strokeWidth="7.8" />
        </g>
        <circle cx="50" cy="71.6" r="9" fill="currentColor" />
      </svg>
      <span className="wordmark-text" data-tauri-drag-region>
        SONORA
      </span>
    </div>
  );
}

/** The import queue's count: progress while a scan runs. Its own component,
 *  so the rest of the sidebar does not render again on every tick. */
function ScanCount() {
  const { scan } = useScan();
  const text = scan && scan.total > 0 ? `${formatCount(scan.done)}/${formatCount(scan.total)}` : '…';
  return <span className="nav-count num t-lime">{text}</span>;
}

export function Sidebar({ active }: { active: View }) {
  const { overview, duplicateGroups, bridge } = useCore();
  const busy = useScanBusy();
  const imports = useImports();
  const mod = bridge.platform === 'macos' ? '⌘' : 'Ctrl+';

  const count = (view: View): { text: string; tone?: 'lime' | 'amber' } | 'scan' | null => {
    if (!overview) return null;
    switch (view) {
      case 'library':
        return { text: formatCount(overview.tracks.total) };
      case 'queue':
        if (busy) return 'scan';
        return overview.tracks.pendingPresent > 0 ? { text: formatCount(overview.tracks.pendingPresent) } : null;
      case 'folders':
        return overview.folders > 0 ? { text: formatCount(overview.folders) } : null;
      case 'attention':
        return overview.attention > 0 ? { text: formatCount(overview.attention), tone: 'amber' } : null;
      case 'duplicates':
        return duplicateGroups ? { text: formatCount(duplicateGroups) } : null;
      default:
        return null;
    }
  };

  const link = (item: NavItem) => {
    const c = count(item.view);
    const isActive = active === item.view || (active === 'track' && item.view === 'library');
    const Icon = item.icon;
    return (
      <li key={item.view}>
        <a
          href={formatRoute(item.route)}
          className={`nav-item ${isActive ? 'is-active' : ''}`}
          aria-current={isActive ? 'page' : undefined}
          data-tip={`${item.label} (${mod}${item.shortcut})`}
        >
          <Icon className="nav-icon" aria-hidden />
          <span className="nav-label">{item.label}</span>
          {c === 'scan' ? <ScanCount /> : c ? <span className={`nav-count num ${c.tone ? `t-${c.tone}` : ''}`}>{c.text}</span> : null}
        </a>
      </li>
    );
  };

  return (
    <nav className="sidebar" aria-label="Main">
      <div className="sidebar-top" data-tauri-drag-region>
        <Wordmark />
      </div>
      <div className="sidebar-import">
        <Menu
          label="Import"
          items={[
            { id: 'folders', label: 'Add folder…', icon: LuFolder, hint: `${mod}⇧O`, onSelect: imports.addFolders },
            { id: 'files', label: 'Add files…', icon: LuPlus, hint: `${mod}O`, onSelect: imports.addFiles },
            { kind: 'separator', id: 'sep' },
            {
              id: 'rescan',
              label: 'Rescan library folders',
              icon: LuHistory,
              disabled: !overview || overview.folders === 0,
              onSelect: () => imports.rescanFolders(),
            },
          ]}
          trigger={(p) => (
            <Button variant="secondary" icon={LuPlus} className="sidebar-import-btn" {...p}>
              Import music
            </Button>
          )}
        />
      </div>
      <div className="sidebar-scroll" data-autoscroll>
        {GROUPS.map((g, i) => (
          <ul key={i} className="nav-group">
            {g.map(link)}
          </ul>
        ))}
        <PlaylistTree />
      </div>
      <ul className="nav-group nav-bottom">
        {link({ view: 'settings', route: { view: 'settings' }, label: 'Settings', icon: LuSettings, shortcut: ',' })}
      </ul>
    </nav>
  );
}
