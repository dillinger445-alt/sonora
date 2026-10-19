/**
 * The playlist tree in the sidebar: folders, playlists and smart
 * playlists. Drag playlists to reorder them or into folders, drag tracks
 * onto a playlist to add them. Keyboard (tree pattern): ↑/↓ move, →/←
 * open and close folders, Enter opens (a folder also in the tree), F2
 * renames, Delete deletes, Alt+↑/↓ moves the playlist among its
 * neighbours.
 */

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import {
  LuChevronRight,
  LuCopy,
  LuEllipsis,
  LuFolder,
  LuFolderOpen,
  LuFolderPlus,
  LuListFilter,
  LuListMusic,
  LuListPlus,
  LuPencil,
  LuSlidersHorizontal,
  LuSparkles,
  LuTrash2,
  LuUsb,
} from 'react-icons/lu';
import type { PlaylistNode } from '../api/types.ts';
import { formatCount } from '../lib/format.ts';
import { formatRoute, navigate, useRoute } from '../lib/router.ts';
import { useDragStart, useDragState, useDropTarget, type DragPayload, type DropHint } from '../state/dnd.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import { Menu, type MenuItem } from './Menu.tsx';

const ROW_INDENT = 14;

function iconFor(n: PlaylistNode, open: boolean) {
  if (n.kind === 'folder') return open ? LuFolderOpen : LuFolder;
  return n.kind === 'smart' ? LuListFilter : LuListMusic;
}

function RenameField({ node, onDone }: { node: PlaylistNode; onDone: (name: string | null) => void }) {
  const [text, setText] = useState(node.name);
  const ref = useRef<HTMLInputElement>(null);
  // Enter and Escape move the focus away, and the blur that follows must
  // not save (or save again).
  const done = useRef(false);
  useLayoutEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    onDone(save && text.trim() && text.trim() !== node.name ? text.trim() : null);
  };
  return (
    <input
      ref={ref}
      className="pl-rename"
      aria-label={`New name for ${node.name}`}
      value={text}
      maxLength={120}
      onChange={(e) => setText(e.target.value)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(true);
        if (e.key === 'Escape') finish(false);
      }}
      onBlur={() => finish(true)}
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
    />
  );
}

export function PlaylistTree() {
  const pl = usePlaylists();
  const route = useRoute();
  const begin = useDragStart();
  const drag = useDragState();
  const [focused, setFocused] = useState<number | null>(null);
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  const menuTriggers = useRef(new Map<number, HTMLButtonElement>());
  const activeId = route.view === 'playlist' ? route.id : null;

  // Visible rows in order, with depth.
  const visible: { n: PlaylistNode; depth: number }[] = [];
  const walk = (parent: number | null, depth: number) => {
    for (const n of pl.childrenOf(parent)) {
      visible.push({ n, depth });
      if (n.kind === 'folder' && pl.expanded.has(n.id)) walk(n.id, depth + 1);
    }
  };
  walk(null, 0);

  // The open playlist's folders open with it.
  useEffect(() => {
    if (activeId == null) return;
    let cur = pl.byId.get(activeId)?.parentId ?? null;
    while (cur != null) {
      pl.setExpanded(cur, true);
      cur = pl.byId.get(cur)?.parentId ?? null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId, pl.byId]);

  const tabId = focused != null && visible.some((v) => v.n.id === focused) ? focused : (activeId ?? visible[0]?.n.id ?? null);
  const focusRow = (id: number) => {
    setFocused(id);
    rowRefs.current.get(id)?.focus();
  };

  // ------------------------------------------------------------- drops
  useDropTarget('playlist-tree', {
    over: (p, { y, hit }) => {
      const row = hit.closest<HTMLElement>('[data-node]');
      if (!row) {
        // Below the last row: to the end of the top level.
        return p.kind === 'node' ? { where: 'after', index: -1 } : null;
      }
      const id = Number(row.dataset.node);
      const n = pl.byId.get(id);
      if (!n) return null;
      const r = row.getBoundingClientRect();
      const f = (y - r.top) / r.height;
      if (p.kind === 'tracks') {
        if (n.kind !== 'playlist' || p.from?.playlistId === id) return null;
        return { where: 'inside', index: id, effect: 'copy' };
      }
      if (p.id === id || pl.isWithin(id, p.id)) return null;
      if (n.kind === 'folder' && f > 0.25 && f < 0.75) return { where: 'inside', index: id, effect: 'move' };
      return { where: f < 0.5 ? 'before' : 'after', index: id, effect: 'move' };
    },
    drop: (p: DragPayload, h: DropHint) => {
      if (p.kind === 'tracks') {
        if (h.index != null) void pl.addTracks(h.index, p.ids);
        return;
      }
      if (h.index === -1) {
        const top = pl.childrenOf(null).filter((x) => x.id !== p.id);
        void pl.move(p.id, null, top.length);
        return;
      }
      const target = h.index != null ? pl.byId.get(h.index) : undefined;
      if (!target) return;
      if (h.where === 'inside') {
        void pl.move(p.id, target.id, pl.childrenOf(target.id).filter((x) => x.id !== p.id).length);
        return;
      }
      const siblings = pl.childrenOf(target.parentId).filter((x) => x.id !== p.id);
      const at = siblings.findIndex((x) => x.id === target.id);
      void pl.move(p.id, target.parentId, h.where === 'before' ? at : at + 1);
    },
  });
  const over = drag?.target === 'playlist-tree' ? drag.hint : null;

  // ---------------------------------------------------------- keyboard
  // A row moved with Alt+↑/↓ changes place in the page, which drops its
  // focus; it gets it back once the new order is shown.
  const refocus = useRef<{ id: number; index: number } | null>(null);
  useLayoutEffect(() => {
    const r = refocus.current;
    if (!r) return;
    const n = pl.byId.get(r.id);
    if (!n) {
      refocus.current = null;
      return;
    }
    if (pl.childrenOf(n.parentId).findIndex((x) => x.id === r.id) !== r.index) return;
    refocus.current = null;
    const el = rowRefs.current.get(r.id);
    const now = document.activeElement;
    if (el && now !== el && (now == null || now === document.body)) el.focus();
  });

  const moveAmong = (n: PlaylistNode, delta: -1 | 1) => {
    const siblings = pl.childrenOf(n.parentId);
    const at = siblings.findIndex((x) => x.id === n.id);
    const to = at + delta;
    if (to < 0 || to >= siblings.length) return;
    refocus.current = { id: n.id, index: to };
    void pl.move(n.id, n.parentId, to);
  };

  /** Shows a playlist, or a folder (opened in the tree too). */
  const openNode = (n: PlaylistNode) => {
    if (n.kind === 'folder') pl.setExpanded(n.id, true);
    navigate({ view: 'playlist', id: n.id });
  };

  const onKey = (e: KeyboardEvent, n: PlaylistNode) => {
    const i = visible.findIndex((v) => v.n.id === n.id);
    const isFolder = n.kind === 'folder';
    const open = pl.expanded.has(n.id);
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        if (e.altKey) moveAmong(n, 1);
        else if (visible[i + 1]) focusRow((visible[i + 1] as { n: PlaylistNode }).n.id);
        break;
      case 'ArrowUp':
        e.preventDefault();
        if (e.altKey) moveAmong(n, -1);
        else if (visible[i - 1]) focusRow((visible[i - 1] as { n: PlaylistNode }).n.id);
        break;
      case 'ArrowRight':
        e.preventDefault();
        if (isFolder && !open) pl.setExpanded(n.id, true);
        else if (isFolder && visible[i + 1]?.n.parentId === n.id) focusRow((visible[i + 1] as { n: PlaylistNode }).n.id);
        break;
      case 'ArrowLeft':
        e.preventDefault();
        if (isFolder && open) pl.setExpanded(n.id, false);
        else if (n.parentId != null) focusRow(n.parentId);
        break;
      case 'Home':
        e.preventDefault();
        if (visible[0]) focusRow(visible[0].n.id);
        break;
      case 'End':
        e.preventDefault();
        if (visible.length) focusRow((visible[visible.length - 1] as { n: PlaylistNode }).n.id);
        break;
      case 'Enter':
        e.preventDefault();
        openNode(n);
        break;
      case 'F2':
        e.preventDefault();
        pl.setRenaming(n.id);
        break;
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        void pl.remove(n.id);
        break;
      case 'ContextMenu':
        e.preventDefault();
        menuTriggers.current.get(n.id)?.click();
        break;
      default:
        break;
    }
  };

  const menuItems = (n: PlaylistNode): MenuItem[] => {
    const items: MenuItem[] = [];
    if (n.kind === 'folder') {
      items.push({ id: 'new', label: 'New playlist inside', icon: LuListPlus, onSelect: () => void pl.create('playlist', { parentId: n.id }) });
      items.push({ id: 'newfolder', label: 'New folder inside', icon: LuFolderPlus, onSelect: () => void pl.create('folder', { parentId: n.id, open: false }) });
      items.push({ kind: 'separator', id: 's0' });
    }
    items.push({ id: 'rename', label: 'Rename', icon: LuPencil, hint: 'F2', onSelect: () => pl.setRenaming(n.id) });
    if (n.kind === 'smart') {
      items.push({ id: 'rules', label: 'Edit rules…', icon: LuSlidersHorizontal, onSelect: () => pl.openRules({ mode: 'edit', id: n.id }) });
      items.push({ id: 'freeze', label: 'Convert to a playlist', icon: LuListMusic, onSelect: () => void pl.freeze(n.id) });
    }
    items.push({ id: 'dup', label: 'Duplicate', icon: LuCopy, onSelect: () => void pl.duplicate(n.id) });
    items.push({ id: 'usb', label: 'Send to USB…', icon: LuUsb, onSelect: () => pl.openUsb([n.id]) });
    items.push({ kind: 'separator', id: 's1' });
    items.push({ id: 'delete', label: 'Delete…', icon: LuTrash2, danger: true, onSelect: () => void pl.remove(n.id) });
    return items;
  };

  const addItems: MenuItem[] = [
    { id: 'playlist', label: 'New playlist', icon: LuListPlus, onSelect: () => void pl.create('playlist') },
    { id: 'smart', label: 'New smart playlist…', icon: LuListFilter, onSelect: () => pl.openRules({ mode: 'create', name: 'New smart playlist', rules: { filter: {}, sort: { key: 'bpm', desc: false } } }) },
    { id: 'folder', label: 'New folder', icon: LuFolderPlus, onSelect: () => void pl.create('folder', { open: false }) },
    { kind: 'separator', id: 's' },
    { id: 'organize', label: 'Sort library into playlists…', icon: LuSparkles, onSelect: () => pl.openOrganize() },
    { id: 'usb', label: 'Send to USB…', icon: LuUsb, disabled: pl.nodes.length === 0, onSelect: () => pl.openUsb() },
  ];

  return (
    <section className="pl" aria-labelledby="pl-heading">
      <div className="pl-head">
        <h2 id="pl-heading" className="pl-title">
          Playlists
        </h2>
        <Menu
          label="New playlist"
          align="end"
          items={addItems}
          trigger={(p) => (
            <button type="button" className="pl-add" aria-label="New playlist, folder or smart playlist" data-tip="New playlist" {...p}>
              <LuListPlus aria-hidden />
            </button>
          )}
        />
      </div>
      {pl.loaded && pl.nodes.length === 0 ? (
        <div className="pl-empty" data-drop="playlist-tree">
          <p>Sort tracks into sets, then send them to a USB drive.</p>
          <button type="button" className="link" onClick={() => void pl.create('playlist')}>
            New playlist
          </button>
        </div>
      ) : (
        <ul className={`pl-tree ${over?.index === -1 ? 'is-drop-end' : ''}`} role="tree" aria-label="Playlists" data-drop="playlist-tree">
          {visible.map(({ n, depth }) => {
            const isFolder = n.kind === 'folder';
            const open = isFolder && pl.expanded.has(n.id);
            const Icon = iconFor(n, open);
            const active = n.id === activeId;
            const hint = over && over.index === n.id ? over : null;
            const dragging = drag?.payload.kind === 'node' && drag.payload.id === n.id;
            return (
              <li key={n.id} role="none">
                <div
                  ref={(el) => {
                    if (el) rowRefs.current.set(n.id, el);
                    else rowRefs.current.delete(n.id);
                  }}
                  role="treeitem"
                  aria-level={depth + 1}
                  aria-expanded={isFolder ? open : undefined}
                  aria-selected={active}
                  aria-current={active ? 'page' : undefined}
                  tabIndex={n.id === tabId ? 0 : -1}
                  data-node={n.id}
                  className={[
                    'pl-row',
                    `is-${n.kind}`,
                    active ? 'is-active' : '',
                    dragging ? 'is-dragged' : '',
                    hint ? `drop-${hint.where}` : '',
                    n.problem ? 'has-problem' : '',
                  ]
                    .filter(Boolean)
                    .join(' ')}
                  style={{ paddingLeft: 8 + depth * ROW_INDENT }}
                  onFocus={() => setFocused(n.id)}
                  onKeyDown={(e) => onKey(e, n)}
                  onClick={() => openNode(n)}
                  onDoubleClick={() => pl.setRenaming(n.id)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    menuTriggers.current.get(n.id)?.click();
                  }}
                  onPointerDown={(e) => {
                    if (pl.renaming === n.id) return;
                    begin(e, () => ({ kind: 'node', id: n.id, label: n.name }));
                  }}
                  title={n.problem ?? undefined}
                >
                  {isFolder ? (
                    <button
                      type="button"
                      tabIndex={-1}
                      className={`pl-chevron ${open ? 'is-open' : ''}`}
                      aria-label={open ? `Close ${n.name}` : `Open ${n.name}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        pl.setExpanded(n.id, !open);
                      }}
                      onPointerDown={(e) => e.stopPropagation()}
                    >
                      <LuChevronRight aria-hidden />
                    </button>
                  ) : (
                    <span className="pl-chevron-slot" aria-hidden />
                  )}
                  <Icon className="pl-icon" aria-hidden />
                  {pl.renaming === n.id ? (
                    <RenameField
                      node={n}
                      onDone={(name) => {
                        pl.setRenaming(null);
                        if (name) void pl.rename(n.id, name);
                        rowRefs.current.get(n.id)?.focus();
                      }}
                    />
                  ) : (
                    <a className="pl-name ellipsis" href={formatRoute({ view: 'playlist', id: n.id })} tabIndex={-1} onClick={(e) => e.preventDefault()} draggable={false}>
                      {n.name}
                    </a>
                  )}
                  {!isFolder && n.count > 0 ? <span className="pl-count num">{formatCount(n.count)}</span> : null}
                  <Menu
                    label={`${n.name} actions`}
                    align="end"
                    items={menuItems(n)}
                    trigger={(p) => (
                      <button
                        type="button"
                        tabIndex={-1}
                        className="pl-more"
                        aria-label={`Actions for ${n.name}`}
                        {...p}
                        ref={(el) => {
                          p.ref(el);
                          if (el) menuTriggers.current.set(n.id, el);
                          else menuTriggers.current.delete(n.id);
                        }}
                        onClick={(e) => {
                          e.stopPropagation();
                          p.onClick();
                        }}
                        onPointerDown={(e) => e.stopPropagation()}
                      >
                        <LuEllipsis aria-hidden />
                      </button>
                    )}
                  />
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
