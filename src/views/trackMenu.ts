/**
 * The menu a track row opens on right-click (or Shift+F10): the same
 * actions as the details panel, for the row or for the selection it is in.
 */

import { LuDisc3, LuExternalLink, LuFolderSearch, LuListMinus, LuListX, LuPause, LuPencil, LuPlay, LuRefreshCw, LuTrash2, LuUsb } from 'react-icons/lu';
import type { Platform } from '../api/bridge.ts';
import type { TrackRow } from '../api/types.ts';
import type { MenuAction, MenuSeparator } from '../components/Menu.tsx';
import { plural } from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import type { TrackActions } from '../state/trackActions.tsx';

export interface TrackMenuOptions {
  /** The tracks it acts on: the selection when the row is part of it, otherwise the row. */
  ids: number[];
  /** The row it was opened on. */
  row: TrackRow;
  actions: TrackActions;
  platform: Platform;
  /** Whether that row's track is the one playing. */
  playing: boolean;
  onPlay: () => void;
  onRemove: (ids: number[], what?: string) => void;
  onTrash: (ids: number[], what?: string) => void;
  /** Inside a playlist: taking the tracks out of it. */
  takeOut?: { name: string; run: (ids: number[]) => void };
}

export function trackMenuItems(o: TrackMenuOptions): (MenuAction | MenuSeparator)[] {
  const { ids, row, actions } = o;
  const one = ids.length === 1;
  const what = one ? `“${row.displayTitle}”` : undefined;
  const mac = o.platform === 'macos';
  const items: (MenuAction | MenuSeparator)[] = [];
  if (one) {
    if (!row.missing) items.push({ id: 'play', label: o.playing ? 'Pause' : 'Play', icon: o.playing ? LuPause : LuPlay, hint: 'P', onSelect: o.onPlay });
    items.push({
      id: 'open',
      label: 'Open analysis',
      icon: LuExternalLink,
      hint: mac ? '↩' : 'Enter',
      disabled: row.status !== 'done',
      onSelect: () => navigate({ view: 'track', id: row.id }),
    });
    if (!row.missing) items.push({ id: 'reveal', label: actions.revealText, icon: LuFolderSearch, onSelect: () => void actions.reveal(row.path) });
    items.push({ kind: 'separator', id: 's1' });
  }
  const takeOut = o.takeOut;
  if (takeOut) items.push({ id: 'take-out', label: `Take out of “${takeOut.name}”`, icon: LuListMinus, onSelect: () => takeOut.run(ids) });
  items.push(
    { id: 'edit', label: 'Edit BPM, key and genre…', icon: LuPencil, onSelect: () => actions.editDj({ ids, row: one ? row : undefined }) },
    { id: 'export', label: 'Export for DJ software…', icon: LuUsb, disabled: one && row.missing, onSelect: () => actions.exportDj(ids) },
    { id: 'discogs', label: 'Look up genres on Discogs…', icon: LuDisc3, onSelect: () => void actions.lookupGenres(ids) },
    {
      id: 'reanalyze',
      label: one ? 'Re-analyze' : `Re-analyze ${plural(ids.length, 'track')}`,
      icon: LuRefreshCw,
      disabled: one && (row.missing || row.status === 'analyzing'),
      onSelect: () => void actions.reanalyze(ids),
    },
    { kind: 'separator', id: 's2' },
    {
      id: 'remove',
      label: one ? 'Remove from library…' : `Remove ${plural(ids.length, 'track')} from the library…`,
      icon: LuListX,
      hint: mac ? '⌫' : 'Del',
      danger: true,
      onSelect: () => o.onRemove(ids, what),
    },
    {
      id: 'trash',
      label: one ? `Move to ${actions.trashName}…` : `Move ${plural(ids.length, 'file')} to the ${actions.trashName}…`,
      icon: LuTrash2,
      hint: mac ? '⌘⌫' : undefined,
      danger: true,
      disabled: one && row.missing,
      onSelect: () => o.onTrash(ids, what),
    },
  );
  return items;
}
