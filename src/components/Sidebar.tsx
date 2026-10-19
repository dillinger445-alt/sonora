import type { ComponentType } from 'react';
import { LuCopy, LuFolder, LuHistory, LuInbox, LuLayoutDashboard, LuLibrary, LuPlus, LuSettings, LuTriangleAlert } from 'react-icons/lu';
import { Button } from './Button.tsx';
import { Menu } from './Menu.tsx';
import { formatCount } from '../lib/format.ts';
import { formatRoute, type Route, type View } from '../lib/router.ts';
import { useCore, useScan } from '../state/core.tsx';
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
];

export const NAV_ORDER: Route[] = [...GROUPS.flat().map((i) => i.route), { view: 'settings' }];

function Wordmark() {
  // Five bars, like the app icon: a level ladder reading left to right.
  return (
    <div className="wordmark" data-tauri-drag-region>
      <svg className="wordmark-glyph" viewBox="0 0 18 14" aria-hidden>
        <rect x="0" y="6" width="2" height="2" rx="0.5" />
        <rect x="4" y="3.5" width="2" height="7" rx="0.5" />
        <rect x="8" y="0" width="2" height="14" rx="0.5" />
        <rect x="12" y="3.5" width="2" height="7" rx="0.5" />
        <rect x="16" y="6" width="2" height="2" rx="0.5" />
      </svg>
      <span className="wordmark-text" data-tauri-drag-region>
        SONORA
      </span>
    </div>
  );
}

export function Sidebar({ active }: { active: View }) {
  const { overview, duplicateGroups, bridge } = useCore();
  const { scan } = useScan();
  const imports = useImports();
  const mod = bridge.platform === 'macos' ? '⌘' : 'Ctrl+';
  const busy = scan != null && scan.state !== 'idle';

  const count = (view: View): { text: string; tone?: 'lime' | 'amber' } | null => {
    if (!overview) return null;
    switch (view) {
      case 'library':
        return { text: formatCount(overview.tracks.total) };
      case 'queue':
        if (busy && scan && scan.total > 0) return { text: `${formatCount(scan.done)}/${formatCount(scan.total)}`, tone: 'lime' };
        if (busy) return { text: '…', tone: 'lime' };
        return overview.tracks.pending > 0 ? { text: formatCount(overview.tracks.pending) } : null;
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
          {c ? <span className={`nav-count num ${c.tone ? `t-${c.tone}` : ''}`}>{c.text}</span> : null}
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
