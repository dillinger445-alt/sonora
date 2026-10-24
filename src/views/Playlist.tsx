/**
 * One playlist: its set at a glance (the timeline), then its tracks in
 * order. Drag rows (or Alt+↑/↓) to reorder, drag them onto another
 * playlist to add them there, Delete to take them out. A smart playlist
 * lists what its rules match, in their order. A folder shows what it holds.
 */

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent, type ReactNode, type RefObject } from 'react';
import {
  LuArrowDownUp,
  LuEllipsis,
  LuFolder,
  LuFolderOpen,
  LuListFilter,
  LuListMusic,
  LuListPlus,
  LuPause,
  LuPencil,
  LuPlay,
  LuSlidersHorizontal,
  LuSparkles,
  LuTrash2,
  LuUsb,
  LuCopy,
} from 'react-icons/lu';
import type { KeyNotation, PlaylistNode, TrackRow } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { ContextMenu, type ContextMenuRequest } from '../components/ContextMenu.tsx';
import { Checkbox } from '../components/Controls.tsx';
import { BpmText, KeyBadge } from '../components/DjMarks.tsx';
import { EmptyState, ErrorState, ViewHeader } from '../components/Layout.tsx';
import { ScoreCell, StatusMark } from '../components/Marks.tsx';
import { Menu, type MenuAction, type MenuItem, type MenuSeparator } from '../components/Menu.tsx';
import { SetTimeline } from '../components/SetTimeline.tsx';
import { focusIsLost } from '../lib/focus.ts';
import { formatCount, formatDuration, formatSpan, plural } from '../lib/format.ts';
import { formatBpm } from '../lib/keys.ts';
import { formatChange, MATCH_LABEL, mixStats, transition } from '../lib/mixing.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { describeRules } from '../lib/rules.ts';
import { useCore } from '../state/core.tsx';
import { useDragStart, useDragState, useDropTarget, type DragPayload, type DropHint } from '../state/dnd.tsx';
import { usePlayer } from '../state/player.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import { useToast } from '../state/toasts.tsx';
import { useTrackActions } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';
import { ArrangeDialog } from './ArrangeDialog.tsx';
import { Inspector } from './Inspector.tsx';
import { trackMenuItems } from './trackMenu.ts';

const ROW = 32;
const OVERSCAN = 10;
/** As `MAX_ARRANGE` in crates/sonora-core/src/playlists.rs. */
const MAX_ARRANGE = 1000;

interface Col {
  id: string;
  label: string;
  width: string;
  min: number;
  end?: boolean;
  hideBelow?: number;
}

const COLUMNS: Col[] = [
  { id: 'pos', label: '#', width: '44px', min: 44, end: true },
  { id: 'title', label: 'Title', width: 'minmax(170px, 2.4fr)', min: 170 },
  { id: 'artist', label: 'Artist', width: 'minmax(100px, 1.3fr)', min: 100, hideBelow: 720 },
  { id: 'bpm', label: 'BPM', width: '54px', min: 54, end: true },
  { id: 'key', label: 'Key', width: '60px', min: 60 },
  { id: 'mix', label: 'Mix in', width: '92px', min: 92 },
  { id: 'genre', label: 'Genre', width: 'minmax(80px, 0.8fr)', min: 80, hideBelow: 900 },
  { id: 'time', label: 'Time', width: '56px', min: 56, end: true },
  { id: 'dj', label: 'DJ', width: '56px', min: 56, end: true, hideBelow: 820 },
];

function MixCell({ prev, row }: { prev: TrackRow | undefined; row: TrackRow }) {
  if (!prev) return <span className="faint">Opener</span>;
  const t = transition(prev, row);
  const tip = `${MATCH_LABEL[t.match]}${t.tempo ? `, ${formatChange(t.tempo.pct)}${t.tempo.halfDouble ? ' at half or double time' : ''}` : ''}`;
  return (
    <span className="mix-cell" data-tip={tip}>
      <span className={`mix-mark is-${t.tone}`} aria-hidden />
      <span className="visually-hidden">{MATCH_LABEL[t.match]}</span>
      <span className={`num ${t.tempo && Math.abs(t.tempo.pct) > 6 ? 't-amber' : 'muted'}`}>{t.tempo ? formatChange(t.tempo.pct) : ''}</span>
      {t.tempo?.halfDouble ? <span className="faint mix-hd">½×2</span> : null}
    </span>
  );
}

interface TableProps {
  /** The grid, for the view to give the keyboard back to it. */
  gridRef: RefObject<HTMLDivElement | null>;
  playlistId: number;
  rows: TrackRow[];
  editable: boolean;
  notation: KeyNotation;
  selection: ReadonlySet<number>;
  onSelection: (s: Set<number>) => void;
  onActive: (r: TrackRow | null) => void;
  active: TrackRow | null;
  onReorder: (order: number[]) => void;
  onRemove: (ids: number[]) => void;
  onAddAt: (ids: number[], index: number) => void;
  scrollTo: { id: number; n: number } | null;
  /** The menu a row opens on right-click or Shift+F10, for `ids` (the selection it is in, or the row). */
  rowMenu: (ids: number[], row: TrackRow) => (MenuAction | MenuSeparator)[];
}

function PlaylistTable({ gridRef, playlistId, rows, editable, notation, selection, onSelection, onActive, active, onReorder, onRemove, onAddAt, scrollTo, rowMenu }: TableProps) {
  const player = usePlayer();
  const begin = useDragStart();
  const drag = useDragState();
  const gridId = useId();
  const bodyRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(500);
  const [width, setWidth] = useState(1000);
  const anchor = useRef<number | null>(null);
  const [menu, setMenu] = useState<ContextMenuRequest | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setHeight(el.clientHeight);
      setWidth(el.clientWidth);
    });
    ro.observe(el);
    setHeight(el.clientHeight);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const cols = COLUMNS.filter((c) => !c.hideBelow || width >= c.hideBelow);
  const template = `30px ${cols.map((c) => c.width).join(' ')}`;
  const index = useMemo(() => new Map(rows.map((r, i) => [r.id, i])), [rows]);
  const activeIndex = active ? (index.get(active.id) ?? -1) : -1;

  const scrollIntoView = useCallback((i: number) => {
    const el = bodyRef.current;
    if (!el) return;
    const top = i * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  }, []);

  useEffect(() => {
    if (!scrollTo) return;
    const i = index.get(scrollTo.id);
    if (i != null) scrollIntoView(i);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollTo]);

  const select = (i: number, e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => {
    const r = rows[i];
    if (!r) return;
    onActive(r);
    if (e.shiftKey && anchor.current != null) {
      const [a, b] = [Math.min(anchor.current, i), Math.max(anchor.current, i)];
      const next = e.metaKey || e.ctrlKey ? new Set(selection) : new Set<number>();
      for (let k = a; k <= b; k += 1) next.add((rows[k] as TrackRow).id);
      onSelection(next);
      return;
    }
    anchor.current = i;
    if (e.metaKey || e.ctrlKey) {
      const next = new Set(selection);
      if (next.has(r.id)) next.delete(r.id);
      else next.add(r.id);
      onSelection(next);
    } else onSelection(new Set([r.id]));
  };

  const moveTo = (i: number, shift: boolean) => {
    if (!rows.length) return;
    const k = Math.max(0, Math.min(rows.length - 1, i));
    select(k, { shiftKey: shift, metaKey: false, ctrlKey: false });
    scrollIntoView(k);
  };

  /** The order with the selected tracks moved by one place. */
  const nudge = (delta: -1 | 1) => {
    if (!editable || selection.size === 0) return;
    const ids = rows.map((r) => r.id);
    const order = [...ids];
    const picked = ids.filter((id) => selection.has(id));
    const first = ids.indexOf(picked[0] as number);
    const end = ids.indexOf(picked[picked.length - 1] as number);
    if ((delta < 0 && first === 0) || (delta > 0 && end === ids.length - 1)) return;
    const rest = order.filter((id) => !selection.has(id));
    const at = Math.max(0, Math.min(rest.length, first + delta));
    rest.splice(at, 0, ...picked);
    onReorder(rest);
    scrollIntoView(Math.max(0, Math.min(rows.length - 1, activeIndex + delta)));
  };

  /** A row outside the selection is selected first; one inside a selection of several acts for all of them. */
  const openRowMenu = (i: number, r: TrackRow, x: number, y: number, keyboard: boolean) => {
    const ids = selection.has(r.id) && selection.size > 1 ? rows.filter((x) => selection.has(x.id)).map((x) => x.id) : [r.id];
    if (ids.length === 1) select(i, { shiftKey: false, metaKey: false, ctrlKey: false });
    setMenu({
      x,
      y,
      keyboard,
      label: ids.length === 1 ? `Actions for ${r.displayTitle}` : `Actions for ${plural(ids.length, 'track')}`,
      items: rowMenu(ids, r),
      restoreFocus: gridRef.current,
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    const mod = e.metaKey || e.ctrlKey;
    const page = Math.max(1, Math.floor(height / ROW) - 1);
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        if (e.altKey) nudge(1);
        else moveTo(activeIndex < 0 ? 0 : activeIndex + 1, e.shiftKey);
        break;
      case 'ArrowUp':
        e.preventDefault();
        if (e.altKey) nudge(-1);
        else moveTo(activeIndex < 0 ? 0 : activeIndex - 1, e.shiftKey);
        break;
      case 'PageDown':
        e.preventDefault();
        moveTo(activeIndex + page, e.shiftKey);
        break;
      case 'PageUp':
        e.preventDefault();
        moveTo(activeIndex - page, e.shiftKey);
        break;
      case 'Home':
        e.preventDefault();
        moveTo(0, e.shiftKey);
        break;
      case 'End':
        e.preventDefault();
        moveTo(rows.length - 1, e.shiftKey);
        break;
      case ' ':
        e.preventDefault();
        if (active) {
          const next = new Set(selection);
          if (next.has(active.id)) next.delete(active.id);
          else next.add(active.id);
          anchor.current = activeIndex;
          onSelection(next);
        }
        break;
      case 'Enter':
        if (active) {
          e.preventDefault();
          if (active.status === 'done') navigate({ view: 'track', id: active.id });
        }
        break;
      case 'Escape':
        if (selection.size) {
          e.preventDefault();
          onSelection(new Set());
        }
        break;
      case 'a':
      case 'A':
        if (mod) {
          e.preventDefault();
          onSelection(new Set(rows.map((r) => r.id)));
        }
        break;
      case 'p':
      case 'P':
        if (!mod && !e.altKey && active && !active.missing) {
          e.preventDefault();
          player.toggle(active);
        }
        break;
      case 'Delete':
      case 'Backspace':
        if (editable && (selection.size || active)) {
          e.preventDefault();
          onRemove(selection.size ? [...selection] : active ? [active.id] : []);
        }
        break;
      case 'ContextMenu':
      case 'F10': {
        // The Menu key, or Shift+F10: the active row's menu, under the row.
        if ((e.key === 'F10' && !e.shiftKey) || !active || activeIndex < 0) break;
        e.preventDefault();
        const box = document.getElementById(`${gridId}-r${activeIndex}`)?.getBoundingClientRect() ?? gridRef.current?.getBoundingClientRect();
        if (box) openRowMenu(activeIndex, active, box.left + 48, box.top + Math.min(box.height, ROW), true);
        break;
      }
      default:
        break;
    }
  };

  // Dropping rows: reorder these tracks, or add others at a place.
  const dropId = `playlist-rows-${playlistId}`;
  useDropTarget(dropId, {
    over: (p, { y }) => {
      if (!editable || p.kind !== 'tracks') return null;
      const el = bodyRef.current;
      if (!el) return null;
      const r = el.getBoundingClientRect();
      const i = Math.max(0, Math.min(rows.length, Math.round((y - r.top + el.scrollTop) / ROW)));
      return { where: 'before', index: i, effect: p.from?.playlistId === playlistId ? 'move' : 'copy' };
    },
    drop: (p: DragPayload, h: DropHint) => {
      if (p.kind !== 'tracks' || h.index == null) return;
      if (p.from?.playlistId !== playlistId) {
        onAddAt(p.ids, h.index);
        return;
      }
      const moving = new Set(p.ids);
      const ids = rows.map((r) => r.id);
      const before = ids.slice(0, h.index).filter((id) => moving.has(id)).length;
      const rest = ids.filter((id) => !moving.has(id));
      const picked = ids.filter((id) => moving.has(id));
      rest.splice(h.index - before, 0, ...picked);
      if (rest.some((id, i) => id !== ids[i])) onReorder(rest);
    },
  });
  const hint = drag?.target === dropId ? drag.hint : null;

  const startDrag = (e: PointerEvent, r: TrackRow) => {
    if ((e.target as HTMLElement).closest('button, input, a')) return;
    begin(e, () => {
      const ids = selection.has(r.id) ? rows.filter((x) => selection.has(x.id)).map((x) => x.id) : [r.id];
      return { kind: 'tracks', ids, label: ids.length === 1 ? r.displayTitle : plural(ids.length, 'track'), from: { playlistId } };
    });
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(rows.length - 1, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);
  const playingId = player.track?.id ?? null;
  const playingActive = player.status === 'playing' || player.status === 'loading';
  const allSelected = rows.length > 0 && selection.size >= rows.length;
  const items: ReactNode[] = [];
  for (let i = first; i <= last; i += 1) {
    const r = rows[i] as TrackRow;
    const sel = selection.has(r.id);
    const isActive = active?.id === r.id;
    const current = r.id === playingId;
    items.push(
      <div
        key={r.id}
        id={`${gridId}-r${i}`}
        role="row"
        aria-rowindex={i + 2}
        aria-selected={sel}
        className={`trow pt-row ${sel ? 'is-selected' : ''} ${isActive ? 'is-active' : ''} ${r.missing ? 'is-missing' : ''} ${current ? 'is-playing' : ''} ${
          drag?.payload.kind === 'tracks' && drag.payload.from?.playlistId === playlistId && drag.payload.ids.includes(r.id) && hint ? 'is-dragged' : ''
        }`}
        style={{ top: i * ROW, gridTemplateColumns: template }}
        onClick={(e: MouseEvent) => {
          gridRef.current?.focus({ preventScroll: true });
          select(i, e);
        }}
        onDoubleClick={() => (r.status === 'done' ? navigate({ view: 'track', id: r.id }) : undefined)}
        onContextMenu={(e) => {
          // Our menu, not the web view's own (Back, Reload).
          e.preventDefault();
          gridRef.current?.focus({ preventScroll: true });
          openRowMenu(i, r, e.clientX, e.clientY, false);
        }}
        onPointerDown={(e) => startDrag(e, r)}
      >
        <div role="gridcell" className="tcell tcell-check" onClick={(e) => e.stopPropagation()}>
          <Checkbox
            checked={sel}
            tabIndex={-1}
            aria-label={`Select ${r.displayTitle}`}
            onChange={() => {
              const next = new Set(selection);
              if (next.has(r.id)) next.delete(r.id);
              else next.add(r.id);
              anchor.current = i;
              onSelection(next);
            }}
          />
        </div>
        {cols.map((c) => (
          <div key={c.id} role="gridcell" className={`tcell tcell-${c.id} ${c.end ? 'is-end' : ''}`}>
            {c.id === 'pos' ? (
              <span className="num pt-pos">{i + 1}</span>
            ) : c.id === 'title' ? (
              <span className="cell-title">
                {r.missing ? (
                  <span className="row-play-slot" aria-hidden />
                ) : (
                  <button
                    type="button"
                    tabIndex={-1}
                    className={`row-play ${current ? 'is-current' : ''}`}
                    aria-label={current && playingActive ? `Pause ${r.displayTitle}` : `Play ${r.displayTitle}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      player.toggle(r);
                    }}
                    onDoubleClick={(e) => e.stopPropagation()}
                  >
                    {current && playingActive ? <LuPause aria-hidden /> : <LuPlay aria-hidden />}
                  </button>
                )}
                <StatusMark track={r} compact />
                <span className="ellipsis" title={r.displayTitle}>
                  {r.displayTitle}
                </span>
              </span>
            ) : c.id === 'artist' ? (
              <span className="ellipsis muted">{r.artist ?? ''}</span>
            ) : c.id === 'bpm' ? (
              <BpmText bpm={r.djBpm} variable={r.tempoVariable} />
            ) : c.id === 'key' ? (
              <KeyBadge keyId={r.djKey} notation={notation} />
            ) : c.id === 'mix' ? (
              <MixCell prev={rows[i - 1]} row={r} />
            ) : c.id === 'genre' ? (
              <span className="ellipsis muted">{r.djGenre ?? ''}</span>
            ) : c.id === 'time' ? (
              <span className="num muted">{formatDuration(r.duration)}</span>
            ) : (
              <ScoreCell score={r.dj} />
            )}
          </div>
        ))}
      </div>,
    );
  }
  const activeElementId = activeIndex >= first && activeIndex <= last ? `${gridId}-r${activeIndex}` : undefined;

  return (
    <>
      <div
        ref={gridRef}
        className="ttable pt"
        role="grid"
        aria-label="Tracks in the playlist"
        aria-rowcount={rows.length + 1}
        aria-multiselectable
        tabIndex={0}
        aria-activedescendant={activeElementId}
        onKeyDown={onKeyDown}
        onFocus={(e) => {
          if (e.target === e.currentTarget && !active && rows[0] && e.currentTarget.matches(':focus-visible')) onActive(rows[0]);
        }}
      >
        <div className="thead" role="rowgroup">
          <div className="trow trow-head" role="row" aria-rowindex={1} style={{ gridTemplateColumns: template }}>
            <div role="columnheader" className="tcell tcell-check">
              <Checkbox
                checked={allSelected}
                indeterminate={selection.size > 0 && !allSelected}
                aria-label={allSelected ? 'Clear selection' : 'Select all tracks'}
                disabled={!rows.length}
                onChange={() => onSelection(allSelected || selection.size ? new Set() : new Set(rows.map((r) => r.id)))}
              />
            </div>
            {cols.map((c) => (
              <div key={c.id} role="columnheader" className={`tcell th ${c.end ? 'is-end' : ''}`}>
                <span className="th-static" data-tip={c.id === 'mix' ? 'How this track mixes in from the one before: the keys on the Camelot wheel and the tempo change' : undefined}>
                  {c.label}
                </span>
              </div>
            ))}
          </div>
        </div>
        <div ref={bodyRef} className="tbody" role="rowgroup" data-drop={dropId} data-autoscroll onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
          <div className="tbody-inner" style={{ height: Math.max(rows.length * ROW, 1) }}>
            {items}
            {hint && hint.index != null ? <div className="pt-insert" style={{ top: hint.index * ROW - 1 }} aria-hidden /> : null}
          </div>
        </div>
      </div>
      <ContextMenu request={menu} onClose={closeMenu} />
    </>
  );
}

function FolderView({ node }: { node: PlaylistNode }) {
  const pl = usePlaylists();
  const kids = pl.childrenOf(node.id);
  return (
    <div className="view view-scroll">
      <ViewHeader
        title={
          <span className="pl-view-title">
            <LuFolderOpen aria-hidden className="pl-view-icon" /> {node.name}
          </span>
        }
        subtitle={kids.length ? `${plural(kids.length, 'item')}, ${plural(node.count, 'track')}` : 'An empty folder'}
        actions={
          <>
            <Button icon={LuListPlus} onClick={() => void pl.create('playlist', { parentId: node.id })}>
              New playlist inside
            </Button>
            <Button variant="primary" icon={LuUsb} disabled={!kids.length} onClick={() => pl.openUsb([node.id])}>
              Send to USB…
            </Button>
          </>
        }
      />
      {kids.length ? (
        <ul className="folder-list">
          {kids.map((k) => {
            const Icon = k.kind === 'folder' ? LuFolder : k.kind === 'smart' ? LuListFilter : LuListMusic;
            return (
              <li key={k.id}>
                <a className="folder-item" href={formatRoute({ view: 'playlist', id: k.id })}>
                  <Icon aria-hidden className="folder-item-icon" />
                  <span className="folder-item-name ellipsis">{k.name}</span>
                  <span className="faint num">
                    {k.kind === 'folder' ? plural(k.children, 'item') : `${plural(k.count, 'track')}${k.duration ? `, ${formatSpan(k.duration)}` : ''}`}
                  </span>
                </a>
              </li>
            );
          })}
        </ul>
      ) : (
        <EmptyState icon={LuFolder} title="Nothing in this folder yet">
          Drag playlists onto it in the sidebar, or make a new one inside.
        </EmptyState>
      )}
    </div>
  );
}

export function PlaylistView({ id }: { id: number }) {
  const pl = usePlaylists();
  const node = pl.byId.get(id);
  if (!node) {
    return (
      <div className="view">
        <ViewHeader title="Playlist" />
        {pl.loaded ? (
          <EmptyState icon={LuListMusic} title="This playlist no longer exists" actions={<Button onClick={() => navigate({ view: 'library' })}>Go to the library</Button>} />
        ) : null}
      </div>
    );
  }
  if (node.kind === 'folder') return <FolderView node={node} />;
  return <TracksView key={node.id} node={node} />;
}

function TracksView({ node }: { node: PlaylistNode }) {
  const { client, settings, bridge } = useCore();
  const pl = usePlaylists();
  const toast = useToast();
  const actions = useTrackActions();
  const player = usePlayer();
  const editable = node.kind === 'playlist';
  const notation = settings?.keyNotation ?? 'camelot';
  const data = useResource(() => client.playlistTracks(node.id), [node.id], { events: ['playlists.changed', 'library.changed', 'scan.finished'], throttleMs: 500 });
  // The order shown right after a change, until the core's answer arrives.
  const [pending, setPending] = useState<number[] | null>(null);
  const [selection, setSelection] = useState<Set<number>>(new Set());
  const [activeAt, setActive] = useState<TrackRow | null>(null);
  const [arranging, setArranging] = useState(false);
  const [scrollTo, setScrollTo] = useState<{ id: number; n: number } | null>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  // After a button in the details panel removes the track it shows, the
  // panel goes: the keyboard goes back to the tracks.
  const focusGrid = useCallback(() => {
    requestAnimationFrame(() => {
      if (focusIsLost()) gridRef.current?.focus({ preventScroll: true });
    });
  }, []);

  useLayoutEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const fetched = data.data?.rows;
  useEffect(() => setPending(null), [fetched]);
  // Tracks that left the playlist (an edit moved them out of a smart one,
  // say) leave the selection too (B40).
  useEffect(() => {
    if (!fetched) return;
    const ids = new Set(fetched.map((r) => r.id));
    setSelection((s) => {
      const kept = [...s].filter((id) => ids.has(id));
      return kept.length === s.size ? s : new Set(kept);
    });
  }, [fetched]);
  const rows = useMemo(() => {
    const base = fetched ?? [];
    if (!pending) return base;
    const by = new Map(base.map((r) => [r.id, r]));
    const out = pending.map((x) => by.get(x)).filter((r): r is TrackRow => r != null);
    // A new order (as many tracks) or tracks taken out (fewer, B95): shown
    // at once. A pending order that no longer fits the tracks is dropped.
    return out.length === pending.length && out.length <= base.length ? out : base;
  }, [fetched, pending]);

  // The cursor's track as loaded now (after an edit or a new analysis), or none once it leaves the playlist.
  const active = useMemo(() => (activeAt ? (rows.find((r) => r.id === activeAt.id) ?? null) : null), [activeAt, rows]);

  const reorder = useCallback(
    (order: number[]) => {
      setPending(order);
      client.reorderPlaylist(node.id, order).catch((e: unknown) => {
        setPending(null);
        data.reload();
        toast({ tone: 'error', title: 'Could not change the order', body: e instanceof Error ? e.message : String(e) });
      });
    },
    [client, node.id, data, toast],
  );

  const remove = useCallback(
    (ids: number[]) => {
      if (!ids.length) return;
      const before = rows.map((r) => r.id);
      setPending(before.filter((x) => !ids.includes(x)));
      setSelection(new Set());
      setActive(null);
      focusGrid();
      void pl.removeTracks(node.id, ids, before);
    },
    [rows, pl, node.id, focusGrid],
  );

  /** Removing tracks from the library (or their files to the Trash): the panel and selection go with them. */
  const removeFromLibrary = useCallback(
    async (how: 'remove' | 'trash', ids: number[], what?: string) => {
      const ok = await (how === 'trash' ? actions.trash : actions.remove)(ids, what);
      if (ok) {
        setSelection(new Set());
        setActive(null);
        focusGrid();
      }
    },
    [actions, focusGrid],
  );

  const playingId = player.track?.id ?? null;
  const playingNow = player.status === 'playing' || player.status === 'loading';
  const togglePlay = player.toggle;
  const rowMenu = useCallback(
    (ids: number[], row: TrackRow) =>
      trackMenuItems({
        ids,
        row,
        actions,
        platform: bridge.platform,
        playing: playingId === row.id && playingNow,
        onPlay: () => togglePlay(row),
        onRemove: (x, what) => void removeFromLibrary('remove', x, what),
        onTrash: (x, what) => void removeFromLibrary('trash', x, what),
        takeOut: editable ? { name: node.name, run: remove } : undefined,
        context: 'playlist',
      }),
    [actions, bridge.platform, playingId, playingNow, togglePlay, removeFromLibrary, editable, node.name, remove],
  );

  const stats = useMemo(() => mixStats(rows), [rows]);
  const bpms = rows.map((r) => r.djBpm).filter((b): b is number => b != null);
  const total = rows.reduce((s, r) => s + (r.duration ?? 0), 0);
  const meta = [
    plural(rows.length, 'track'),
    total > 0 ? formatSpan(total) : null,
    bpms.length && !(node.kind === 'smart' && (node.rules?.filter.bpmMin != null || node.rules?.filter.bpmMax != null))
      ? Math.min(...bpms) === Math.max(...bpms)
        ? `${formatBpm(bpms[0])} BPM`
        : `${formatBpm(Math.min(...bpms))}–${formatBpm(Math.max(...bpms))} BPM`
      : null,
    rows.length > 1 ? `${formatCount(stats.harmonic)} of ${formatCount(stats.transitions)} changes in mixing keys` : null,
  ].filter(Boolean);

  const more: MenuItem[] = [
    { id: 'rename', label: 'Rename', icon: LuPencil, onSelect: () => pl.setRenaming(node.id) },
    ...(node.kind === 'smart'
      ? [
          { id: 'rules', label: 'Edit rules…', icon: LuSlidersHorizontal, onSelect: () => pl.openRules({ mode: 'edit', id: node.id }) } as MenuItem,
          { id: 'freeze', label: 'Convert to a playlist', icon: LuListMusic, onSelect: () => void pl.freeze(node.id) } as MenuItem,
        ]
      : []),
    { id: 'dup', label: 'Duplicate', icon: LuCopy, onSelect: () => void pl.duplicate(node.id) },
    { id: 'export', label: 'Export for DJ software…', icon: LuUsb, disabled: !rows.length, onSelect: () => actions.exportDj(rows.map((r) => r.id)) },
    { id: 'sort', label: 'Sort these into playlists…', icon: LuSparkles, disabled: !rows.length, onSelect: () => pl.openOrganize({ ids: rows.map((r) => r.id) }) },
    { kind: 'separator', id: 's' },
    { id: 'delete', label: 'Delete…', icon: LuTrash2, danger: true, onSelect: () => void pl.remove(node.id) },
  ];

  const where = pl.pathOf(node.id);
  const Icon = node.kind === 'smart' ? LuListFilter : LuListMusic;
  const showInspector = active != null || selection.size > 1;

  let body: ReactNode;
  if (data.error && !data.data) body = <ErrorState title="The playlist could not be loaded" error={data.error} onRetry={data.reload} />;
  else if (!data.data) body = null;
  else if (!rows.length)
    body = (
      <div className="pt-empty">
        <EmptyState
          icon={Icon}
          title={editable ? 'No tracks yet' : 'Nothing matches these rules yet'}
          actions={editable ? <Button onClick={() => navigate({ view: 'library' })}>Go to the library</Button> : undefined}
        >
          {editable ? (
            <>In the library, drag tracks onto “{node.name}” in the sidebar, or select them and choose Add to playlist.</>
          ) : (
            <>Tracks that match appear here by themselves. Change the rules to widen it.</>
          )}
        </EmptyState>
      </div>
    );
  else
    body = (
      <div className={`browser-body ${showInspector ? 'with-inspector' : ''}`}>
        <div className="browser-table">
          <PlaylistTable
            gridRef={gridRef}
            playlistId={node.id}
            rows={rows}
            editable={editable}
            notation={notation}
            selection={selection}
            onSelection={setSelection}
            onActive={setActive}
            active={active}
            onReorder={reorder}
            onRemove={remove}
            onAddAt={(ids, index) => void pl.addTracks(node.id, ids, index)}
            scrollTo={scrollTo}
            rowMenu={rowMenu}
          />
        </div>
        {showInspector ? (
          <Inspector
            row={active}
            selectionCount={selection.size}
            selectedIds={[...selection]}
            actions={actions}
            onRemove={(ids, what) => removeFromLibrary('remove', ids, what)}
            onTrash={(ids, what) => removeFromLibrary('trash', ids, what)}
            onClearSelection={() => setSelection(new Set())}
            playlist={editable ? { id: node.id, name: node.name, remove } : undefined}
          />
        ) : null}
      </div>
    );

  return (
    <div className="view view-fill">
      <ViewHeader
        title={
          <span className="pl-view-title">
            <Icon aria-hidden className="pl-view-icon" /> {node.name}
          </span>
        }
        subtitle={
          <span className="pl-view-meta">
            {where ? <span className="faint">{where} / </span> : null}
            {node.kind === 'smart' ? <span>{describeRules(node.rules, notation)}. </span> : null}
            {meta.join(', ')}
          </span>
        }
        actions={
          <>
            {editable ? (
              <Button
                icon={LuArrowDownUp}
                disabled={rows.length < 3 || rows.length > MAX_ARRANGE}
                onClick={() => setArranging(true)}
                data-tip={
                  rows.length > MAX_ARRANGE
                    ? `Arranging works on playlists of up to ${formatCount(MAX_ARRANGE)} tracks`
                    : 'Order the tracks so neighbours mix: keys that fit on the Camelot wheel, small tempo changes'
                }
              >
                Arrange for mixing…
              </Button>
            ) : (
              <Button icon={LuSlidersHorizontal} onClick={() => pl.openRules({ mode: 'edit', id: node.id })}>
                Edit rules…
              </Button>
            )}
            <Button variant="primary" icon={LuUsb} disabled={!rows.length} onClick={() => pl.openUsb([node.id])}>
              Send to USB…
            </Button>
            <Menu
              label="More playlist actions"
              align="end"
              items={more}
              trigger={(p) => (
                <button type="button" className="icon-btn" aria-label="More playlist actions" data-tip="More" {...p}>
                  <LuEllipsis aria-hidden />
                </button>
              )}
            />
          </>
        }
      />
      <div ref={wrap} className="pl-timeline">
        {rows.length ? (
          <SetTimeline
            rows={rows}
            notation={notation}
            width={width}
            selected={selection}
            label={`The set: ${plural(rows.length, 'track')}, ${formatCount(stats.harmonic)} of ${formatCount(stats.transitions)} changes in keys that mix`}
            onPick={(trackId) => {
              const r = rows.find((x) => x.id === trackId);
              if (!r) return;
              setSelection(new Set([trackId]));
              setActive(r);
              setScrollTo({ id: trackId, n: Date.now() });
            }}
          />
        ) : null}
      </div>
      {body}
      {actions.dialogs}
      <ArrangeDialog open={arranging} playlistId={node.id} rows={rows} notation={notation} onClose={() => setArranging(false)} onApply={reorder} />
    </div>
  );
}
