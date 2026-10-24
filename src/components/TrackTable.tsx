/**
 * Virtualized, keyboard-driven track table. Rows are fetched from the core
 * in pages as they scroll into view, so libraries of any size stay fast.
 *
 * Keyboard (the grid itself takes focus): ↑/↓ move, Shift extends, Space
 * toggles, Enter opens, P plays or pauses, ⌘/Ctrl+A selects all, Esc
 * clears, Delete asks to remove.
 *
 * The active row, the selection and the Shift anchor are tracked by track id,
 * not by position: rows re-sort while a scan updates them, and the active
 * track (the one the details panel shows) must stay the same track.
 */

import {
  memo,
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
  type Ref,
} from 'react';
import { LuArrowDown, LuArrowUp } from 'react-icons/lu';
import type { LibraryFilter, ScoreKind, Sort, TrackRow } from '../api/types.ts';
import { plural } from '../lib/format.ts';
import { reuseRows } from '../lib/rows.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { useDragStart } from '../state/dnd.tsx';
import { throttle } from '../state/emitter.ts';
import { usePlayer } from '../state/player.tsx';
import { useToast } from '../state/toasts.tsx';
import { ContextMenu, type ContextMenuRequest } from './ContextMenu.tsx';
import { Checkbox } from './Controls.tsx';
import { ErrorState } from './Layout.tsx';
import type { MenuAction, MenuSeparator } from './Menu.tsx';
import { ASCENDING_FIRST, fitColumns, trackColumns, type Column, type ColumnContext } from './trackColumns.tsx';

const ROW = 32;
const PAGE = 200;
const OVERSCAN = 12;

export interface TrackTableHandle {
  /** Moves keyboard focus to the table. */
  focus: () => void;
  /** Reports removed tracks: if the active one is among them, the row that takes its place becomes active. */
  removed: (ids: number[]) => void;
}

export interface TrackTableProps {
  filter: LibraryFilter;
  sort: Sort;
  onSort: (s: Sort) => void;
  scoreKind: ScoreKind;
  selection: ReadonlySet<number>;
  onSelection: (s: Set<number>) => void;
  /** The track with keyboard focus (drives the details panel). */
  onActive?: (row: TrackRow | null) => void;
  onOpen: (row: TrackRow) => void;
  /** `label` names a single track for the confirmation. */
  onDelete?: (ids: number[], label?: string) => void;
  /** ⌘⌫ on macOS, as in Finder: move the files to the Trash (after asking). */
  onTrash?: (ids: number[], label?: string) => void;
  /** The menu a row opens on right-click or Shift+F10, for `ids` (the selection it is in, or the row). */
  rowMenu?: (ids: number[], row: TrackRow) => (MenuAction | MenuSeparator)[];
  onTotal?: (total: number) => void;
  /** Shown when the query has no rows. */
  empty: ReactNode;
  label: string;
  /** Core events that make the rows stale. */
  refreshOn?: string[];
  handle?: Ref<TrackTableHandle>;
}

/** What a row does with the pointer; the table answers with its latest state. */
interface RowEvents {
  click: (e: MouseEvent, i: number, r: TrackRow) => void;
  toggle: (i: number, r: TrackRow) => void;
  open: (r: TrackRow) => void;
  menu: (e: MouseEvent, i: number, r: TrackRow) => void;
  pointerDown: (e: PointerEvent, r: TrackRow) => void;
}

interface TableRowProps {
  index: number;
  row: TrackRow;
  id: string;
  selected: boolean;
  active: boolean;
  playing: boolean;
  template: string;
  columns: Column[];
  ctx: ColumnContext;
  events: RowEvents;
}

/**
 * One row. Memoised: while a scan updates tracks, or the rows are fetched
 * again with nothing changed, only rows whose track (or selection, focus,
 * playback) changed render again, not every row on screen.
 */
const TableRow = memo(function TableRow({ index, row: r, id, selected, active, playing, template, columns, ctx, events }: TableRowProps) {
  return (
    <div
      id={id}
      role="row"
      aria-rowindex={index + 2}
      aria-selected={selected}
      className={`trow ${selected ? 'is-selected' : ''} ${active ? 'is-active' : ''} ${r.missing ? 'is-missing' : ''} ${playing ? 'is-playing' : ''}`}
      style={{ top: index * ROW, gridTemplateColumns: template }}
      onClick={(e) => events.click(e, index, r)}
      onDoubleClick={() => events.open(r)}
      onContextMenu={(e) => events.menu(e, index, r)}
      onPointerDown={(e) => events.pointerDown(e, r)}
    >
      <div role="gridcell" className="tcell tcell-check" onClick={(e) => e.stopPropagation()}>
        <Checkbox checked={selected} tabIndex={-1} aria-label={`Select ${r.displayTitle}`} onChange={() => events.toggle(index, r)} />
      </div>
      {columns.map((c) => (
        <div key={c.id} role="gridcell" className={`tcell tcell-${c.id} ${c.align === 'end' ? 'is-end' : ''}`}>
          {c.render(r, ctx)}
        </div>
      ))}
    </div>
  );
});

type PendingMove =
  /** A keyboard move to a row that has not loaded yet. */
  | { kind: 'index'; index: number; extend: boolean }
  /**
   * After the active track was removed: the track that takes its place (the
   * next one that stays, else the previous one), once fresh rows arrive.
   * `fallback` is the position it should end up at.
   */
  | { kind: 'replace'; id: number | null; fallback: number };

export function TrackTable({
  filter,
  sort,
  onSort,
  scoreKind,
  selection,
  onSelection,
  onActive,
  onOpen,
  onDelete,
  onTrash,
  rowMenu,
  onTotal,
  empty,
  label,
  refreshOn = ['library.changed', 'scan.finished', 'library.indexed'],
  handle,
}: TrackTableProps) {
  const { client, settings } = useCore();
  const player = usePlayer();
  const toast = useToast();
  const beginDrag = useDragStart();
  const gridId = useId();
  const gridRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  const [height, setHeight] = useState(600);
  const [scrollTop, setScrollTop] = useState(0);
  const [total, setTotal] = useState<number | null>(null);
  const [version, setVersion] = useState(0);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** Cursor position (row index). */
  const [active, setActive] = useState(-1);
  /** The active track; follows it when rows move. */
  const [activeRow, setActiveRow] = useState<TrackRow | null>(null);
  const anchor = useRef<{ index: number; id: number | null }>({ index: -1, id: null });
  const [menu, setMenu] = useState<ContextMenuRequest | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const pending = useRef<PendingMove | null>(null);

  const cache = useRef(new Map<number, TrackRow[]>());
  const stale = useRef(new Map<number, TrackRow[]>());
  /** `stale` holds the previous query's rows (shown until the new ones come). */
  const foreign = useRef(false);
  /** A new query whose first page will prune the selection and the active track. */
  const prune = useRef<string | null>(null);
  const inflight = useRef(new Set<number>());
  const generation = useRef(0);
  const bump = useCallback(() => setVersion((v) => v + 1), []);

  const key = JSON.stringify([filter, sort]);
  const filterRef = useRef(filter);
  const sortRef = useRef(sort);
  const keyRef = useRef(key);
  const onActiveRef = useRef(onActive);
  const selectionRef = useRef(selection);
  const latest = useRef({ active, activeRow, total });
  // Runs before the effects below, so they always see the latest values.
  useLayoutEffect(() => {
    filterRef.current = filter;
    sortRef.current = sort;
    keyRef.current = key;
    onActiveRef.current = onActive;
    selectionRef.current = selection;
    latest.current = { active, activeRow, total };
  });

  const fetchPage = useCallback(
    (p: number) => {
      if (inflight.current.has(p)) return;
      const gen = generation.current;
      inflight.current.add(p);
      client
        .query(filterRef.current, sortRef.current, p * PAGE, PAGE)
        .then(
          (res) => {
            if (gen !== generation.current) return;
            // The previous query's rows were only stand-ins until now.
            if (foreign.current) {
              stale.current = new Map();
              foreign.current = false;
            }
            // Tracks that did not change keep their objects, so their rows
            // do not render again.
            const before = cache.current.get(p) ?? stale.current.get(p);
            cache.current.set(p, before ? reuseRows(before, res.rows) : res.rows);
            stale.current.delete(p);
            setTotal(res.total);
            setLoadError(null);
            bump();
          },
          (e: unknown) => {
            if (gen !== generation.current) return;
            if (foreign.current) {
              stale.current = new Map();
              foreign.current = false;
              setTotal(null);
            }
            setLoadError(e instanceof Error ? e.message : String(e));
            // A move waiting for these rows cannot finish; keep the keyboard usable.
            if (pending.current?.kind === 'replace') setActiveRow(null);
            pending.current = null;
          },
        )
        .finally(() => {
          if (gen === generation.current) inflight.current.delete(p);
        });
    },
    [client, bump],
  );

  const rowAt = useCallback((i: number): TrackRow | undefined => {
    const p = Math.floor(i / PAGE);
    const page = cache.current.get(p) ?? stale.current.get(p);
    return page?.[i - p * PAGE];
  }, []);

  const freshRowAt = useCallback((i: number): TrackRow | undefined => {
    const p = Math.floor(i / PAGE);
    return cache.current.get(p)?.[i - p * PAGE];
  }, []);

  /**
   * Where a track currently is among the loaded rows (fresh rows first,
   * unless `freshOnly`). Positions beyond the current total are ignored.
   */
  const locate = useCallback((id: number, freshOnly = false): { index: number; row: TrackRow } | null => {
    const n = latest.current.total;
    for (const store of freshOnly ? [cache.current] : [cache.current, stale.current]) {
      for (const [p, rows] of store) {
        const i = rows.findIndex((r) => r.id === id);
        if (i >= 0 && (n == null || p * PAGE + i < n)) return { index: p * PAGE + i, row: rows[i] as TrackRow };
      }
    }
    return null;
  }, []);

  // New query: start over at the top. The rows on screen stay until the new
  // ones arrive, so the table does not blank on every keystroke and a click
  // in between lands on a row; the selection and the active track keep what
  // is in the new results (B24).
  useLayoutEffect(() => {
    generation.current += 1;
    stale.current = new Map([...stale.current, ...cache.current]);
    foreign.current = true;
    cache.current = new Map();
    inflight.current = new Set();
    setLoadError(null);
    setActive(-1);
    anchor.current = { index: -1, id: null };
    pending.current = null;
    prune.current = key;
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    setScrollTop(0);
    fetchPage(0);
  }, [key, fetchPage]);

  // Same query, fresher data: the rows on screen stay until their new
  // versions arrive; other pages are dropped (and refetched when scrolled to),
  // so nothing outdated lingers out of sight.
  const softReload = useCallback(() => {
    generation.current += 1;
    const first = Math.max(0, Math.floor(scrollRef.current ? scrollRef.current.scrollTop / ROW : 0) - OVERSCAN);
    const last = first + Math.ceil((scrollRef.current?.clientHeight ?? 600) / ROW) + 2 * OVERSCAN;
    const keep = new Map<number, TrackRow[]>();
    for (let p = Math.floor(first / PAGE); p <= Math.floor(last / PAGE); p += 1) {
      const rows = cache.current.get(p) ?? stale.current.get(p);
      if (rows) keep.set(p, rows);
    }
    stale.current = keep;
    cache.current = new Map();
    inflight.current = new Set();
    for (let p = Math.floor(first / PAGE); p <= Math.floor(last / PAGE); p += 1) fetchPage(p);
  }, [fetchPage]);

  const reloadThrottle = useMemo(() => throttle(softReload, 1200), [softReload]);
  useEffect(() => () => reloadThrottle.cancel(), [reloadThrottle]);
  useCoreEvent(refreshOn, () => reloadThrottle.run());

  // A finished track: patch it in place now; re-sorting follows in the next reload.
  useCoreEvent(['scan.item'], (_n, payload) => {
    const row = (payload as { track: TrackRow | null }).track;
    if (!row) return;
    let hit = false;
    for (const store of [cache.current, stale.current]) {
      for (const rows of store.values()) {
        const i = rows.findIndex((r) => r.id === row.id);
        if (i >= 0) {
          rows[i] = row;
          hit = true;
        }
      }
    }
    if (hit) bump();
    else if (latest.current.activeRow?.id === row.id) setActiveRow(row);
    reloadThrottle.run();
  });

  useEffect(() => {
    if (total != null) onTotal?.(total);
  }, [total, onTotal]);

  // Measure.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setWidth(el.clientWidth);
      setHeight(el.clientHeight);
    });
    ro.observe(el);
    setWidth(el.clientWidth);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  const count = total ?? 0;
  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(count - 1, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);

  // Fetch pages that scrolled into view.
  useEffect(() => {
    if (total == null || count === 0) return;
    for (let p = Math.floor(first / PAGE); p <= Math.floor(last / PAGE); p += 1) {
      if (!cache.current.has(p)) fetchPage(p);
    }
  }, [first, last, total, count, fetchPage, version]);

  const columns = useMemo(() => fitColumns(trackColumns(scoreKind), width), [scoreKind, width]);
  const template = `36px ${columns.map((c) => c.width).join(' ')}`;
  const notation = settings?.keyNotation ?? 'camelot';
  const playingId = player.track?.id ?? null;
  const playingActive = player.status === 'playing' || player.status === 'loading';
  const toggle = player.toggle;
  const cellCtx = useMemo<ColumnContext>(
    () => ({ scoreKind, notation, playing: playingId != null ? { id: playingId, active: playingActive } : null, onPlay: toggle }),
    [scoreKind, notation, playingId, playingActive, toggle],
  );

  // Report the active track.
  useEffect(() => {
    onActiveRef.current?.(activeRow);
  }, [activeRow]);

  const scrollIntoView = (i: number) => {
    const el = scrollRef.current;
    if (!el) return;
    const top = i * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  /** Ids for rows `a..=b` in the current order (asks the core unless every row is freshly loaded). */
  const idsBetween = async (a: number, b: number): Promise<number[]> => {
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    const out: number[] = [];
    let missing = false;
    for (let i = lo; i <= hi; i += 1) {
      const r = freshRowAt(i);
      if (!r) {
        missing = true;
        break;
      }
      out.push(r.id);
    }
    if (!missing) return out;
    const ids: number[] = [];
    for (let off = lo; off <= hi; off += 2000) {
      const res = await client.query(filterRef.current, sortRef.current, off, Math.min(2000, hi - off + 1));
      ids.push(...res.rows.map((r) => r.id));
    }
    return ids;
  };

  const selectRange = async (to: number, additive: boolean) => {
    const a = anchor.current;
    const anchorAt = a.id != null ? locate(a.id) : null;
    const from = anchorAt ? anchorAt.index : a.index >= 0 ? a.index : to;
    const query = keyRef.current;
    try {
      const ids = await idsBetween(from, to);
      // The query changed while the rows were fetched: these ids no longer apply.
      if (query !== keyRef.current) return;
      const next = additive ? new Set(selectionRef.current) : new Set<number>();
      for (const id of ids) next.add(id);
      onSelection(next);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not select those tracks', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const selectAll = () => {
    const query = keyRef.current;
    client.ids(filterRef.current).then(
      (r) => {
        if (query === keyRef.current) onSelection(new Set(r.ids));
      },
      (e: unknown) => toast({ tone: 'error', title: 'Could not select every track', body: e instanceof Error ? e.message : String(e) }),
    );
  };

  /** Makes row `i` (track `r`) active; selects it, or extends the selection to it. */
  const activate = (i: number, r: TrackRow, extend: boolean) => {
    setActive(i);
    setActiveRow(r);
    if (extend) {
      void selectRange(i, false);
    } else {
      anchor.current = { index: i, id: r.id };
      onSelection(new Set([r.id]));
    }
  };

  const moveTo = (i: number, e: { shiftKey: boolean }) => {
    if (count === 0) return;
    const idx = Math.max(0, Math.min(count - 1, i));
    setActive(idx);
    scrollIntoView(idx);
    const r = rowAt(idx);
    if (r) {
      pending.current = null;
      activate(idx, r, e.shiftKey);
    } else {
      // Not loaded yet: finish the move when its page arrives.
      pending.current = { kind: 'index', index: idx, extend: e.shiftKey };
    }
  };

  // After new rows arrive: finish a pending move, or follow the active track
  // to its new position.
  useLayoutEffect(() => {
    const p = pending.current;
    const { active: cur, activeRow: row, total: n } = latest.current;
    // A new query's first page: keep only what is in the new results.
    const first = cache.current.get(0);
    if (prune.current != null && prune.current === keyRef.current && first && n != null) {
      const query = prune.current;
      prune.current = null;
      const keep = (ids: Set<number>) => {
        if (query !== keyRef.current) return;
        const sel = selectionRef.current;
        const next = new Set([...sel].filter((id) => ids.has(id)));
        if (next.size !== sel.size) onSelection(next);
        const r = latest.current.activeRow;
        if (r && !ids.has(r.id)) {
          setActiveRow(null);
          setActive(-1);
        }
      };
      if (n <= first.length) keep(new Set(first.map((r) => r.id)));
      else if (selectionRef.current.size > 0 || row) {
        client.ids(filterRef.current).then(
          (r) => keep(new Set(r.ids)),
          () => undefined,
        );
      }
    }
    if (p?.kind === 'index') {
      const idx = n != null ? Math.min(p.index, n - 1) : p.index;
      const r = idx >= 0 ? rowAt(idx) : undefined;
      if (r) {
        pending.current = null;
        activate(idx, r, p.extend);
      }
      return;
    }
    if (p?.kind === 'replace') {
      // Only fresh rows count: the stale ones still contain the removed tracks.
      if (cache.current.size === 0 || n == null) return;
      if (n === 0) {
        pending.current = null;
        setActive(-1);
        setActiveRow(null);
        return;
      }
      const hit = p.id != null ? locate(p.id, true) : null;
      if (hit) {
        pending.current = null;
        activate(hit.index, hit.row, false);
        return;
      }
      const idx = Math.min(p.fallback, n - 1);
      const r = freshRowAt(idx);
      if (r) {
        pending.current = null;
        activate(idx, r, false);
      } else if (!cache.current.has(Math.floor(idx / PAGE))) {
        fetchPage(Math.floor(idx / PAGE));
      }
      return;
    }
    if (!row) return;
    const loc = locate(row.id);
    if (!loc) return;
    if (loc.row !== row) setActiveRow(loc.row);
    if (loc.index !== cur) setActive(loc.index);
    // `activate` and the lookups read refs; only new rows matter here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  useImperativeHandle(
    handle,
    () => ({
      focus: () => gridRef.current?.focus({ preventScroll: true }),
      removed: (ids: number[]) => {
        const row = latest.current.activeRow;
        if (row && ids.includes(row.id)) {
          const gone = new Set(ids);
          const at = locate(row.id)?.index ?? Math.max(0, latest.current.active);
          // The track that takes the removed one's place: the next one that
          // stays, else the previous one.
          let next: number | null = null;
          for (let i = at + 1; next == null; i += 1) {
            const r = rowAt(i);
            if (!r) break;
            if (!gone.has(r.id)) next = r.id;
          }
          for (let i = at - 1; next == null && i >= 0; i -= 1) {
            const r = rowAt(i);
            if (!r) break;
            if (!gone.has(r.id)) next = r.id;
          }
          // Where it will be: removed rows above the active one move it up.
          let above = 0;
          for (const id of ids) {
            const loc = locate(id);
            if (loc && loc.index < at) above += 1;
          }
          pending.current = { kind: 'replace', id: next, fallback: Math.max(0, at - above) };
        }
        softReload();
      },
    }),
    [softReload, locate, rowAt],
  );

  /**
   * Opens the row menu at a point. A row outside the selection is selected
   * first (as in Finder); one inside a selection of several acts for all of them.
   */
  const openRowMenu = (i: number, r: TrackRow, x: number, y: number, keyboard: boolean) => {
    if (!rowMenu) return;
    const sel = selectionRef.current;
    let ids: number[];
    if (sel.has(r.id) && sel.size > 1) {
      ids = [...sel];
    } else {
      ids = [r.id];
      pending.current = null;
      setActive(i);
      setActiveRow(r);
      anchor.current = { index: i, id: r.id };
      onSelection(new Set([r.id]));
    }
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
    // Keys pressed on the column headers or row checkboxes keep their own meaning.
    if (e.target !== e.currentTarget) return;
    const pageRows = Math.max(1, Math.floor(height / ROW) - 1);
    const mod = e.metaKey || e.ctrlKey;
    const settled = pending.current == null;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        moveTo(active < 0 ? 0 : active + 1, e);
        break;
      case 'ArrowUp':
        e.preventDefault();
        moveTo(active < 0 ? 0 : active - 1, e);
        break;
      case 'PageDown':
        e.preventDefault();
        moveTo(active + pageRows, e);
        break;
      case 'PageUp':
        e.preventDefault();
        moveTo(active - pageRows, e);
        break;
      case 'Home':
        e.preventDefault();
        moveTo(0, e);
        break;
      case 'End':
        e.preventDefault();
        moveTo(count - 1, e);
        break;
      case ' ': {
        e.preventDefault();
        if (!settled || !activeRow) break;
        const next = new Set(selection);
        if (next.has(activeRow.id)) next.delete(activeRow.id);
        else next.add(activeRow.id);
        anchor.current = { index: active, id: activeRow.id };
        onSelection(next);
        break;
      }
      case 'Enter':
        if (settled && activeRow) {
          e.preventDefault();
          onOpen(activeRow);
        }
        break;
      case 'Escape':
        if (selection.size > 0) {
          e.preventDefault();
          onSelection(new Set());
        }
        break;
      case 'Delete':
      case 'Backspace': {
        // ⌘⌫ moves to the Trash (as in Finder); Delete or ⌫ alone removes
        // from the library only.
        const act = e.metaKey && e.key === 'Backspace' ? onTrash : onDelete;
        if (!act || !settled) break;
        // The same tracks the details panel shows: the selection when it
        // holds several, otherwise the active track.
        e.preventDefault();
        if (selection.size > 1) act([...selection]);
        else if (activeRow) act([activeRow.id], `“${activeRow.displayTitle}”`);
        else if (selection.size === 1) act([...selection]);
        break;
      }
      case 'a':
      case 'A':
        if (mod) {
          e.preventDefault();
          selectAll();
        }
        break;
      case 'p':
      case 'P':
        if (!mod && !e.altKey && settled && activeRow && !activeRow.missing) {
          e.preventDefault();
          player.toggle(activeRow);
        }
        break;
      case 'ContextMenu':
      case 'F10': {
        // The Menu key, or Shift+F10: the active row's menu, under the row.
        if (e.key === 'F10' && !e.shiftKey) break;
        if (!settled || !activeRow || active < 0) break;
        e.preventDefault();
        const el = document.getElementById(`${gridId}-r${active}`);
        const box = el?.getBoundingClientRect() ?? gridRef.current?.getBoundingClientRect();
        if (box) openRowMenu(active, activeRow, box.left + 48, box.top + Math.min(box.height, ROW), true);
        break;
      }
      default:
        break;
    }
  };

  const onRowClick = (e: MouseEvent, i: number, r: TrackRow) => {
    gridRef.current?.focus({ preventScroll: true });
    pending.current = null;
    setActive(i);
    setActiveRow(r);
    if (e.shiftKey) {
      void selectRange(i, e.metaKey || e.ctrlKey);
      return;
    }
    anchor.current = { index: i, id: r.id };
    if (e.metaKey || e.ctrlKey) {
      const next = new Set(selection);
      if (next.has(r.id)) next.delete(r.id);
      else next.add(r.id);
      onSelection(next);
    } else {
      onSelection(new Set([r.id]));
    }
  };

  const toggleOne = (i: number, r: TrackRow) => {
    gridRef.current?.focus({ preventScroll: true });
    pending.current = null;
    setActive(i);
    setActiveRow(r);
    anchor.current = { index: i, id: r.id };
    const next = new Set(selection);
    if (next.has(r.id)) next.delete(r.id);
    else next.add(r.id);
    onSelection(next);
  };

  const onFocus = (e: FocusEvent<HTMLDivElement>) => {
    // Tabbing onto the grid itself with nothing active starts at the top. A
    // click (which focuses the grid first) activates the clicked row instead.
    if (e.target !== e.currentTarget || activeRow || active >= 0 || pending.current || count === 0) return;
    if (!e.currentTarget.matches(':focus-visible')) return;
    const r = freshRowAt(0);
    setActive(0);
    if (r) setActiveRow(r);
  };

  const retry = () => {
    setLoadError(null);
    softReload();
  };

  const allSelected = count > 0 && selection.size >= count;
  const someSelected = selection.size > 0 && !allSelected;

  const headerClick = (c: Column) => {
    if (!c.sortKey) return;
    if (sort.key === c.sortKey) onSort({ key: c.sortKey, desc: !sort.desc });
    else onSort({ key: c.sortKey, desc: !ASCENDING_FIRST.has(c.sortKey) });
  };

  // Rows call the table through one stable object, so they can skip
  // rendering; it reaches the handlers of the latest render.
  const handlers = useRef<RowEvents | null>(null);
  const latestHandlers: RowEvents = {
    click: onRowClick,
    toggle: toggleOne,
    open: onOpen,
    menu: (e, i, r) => {
      if (!rowMenu) return;
      // Our menu, not the web view's own (Back, Reload).
      e.preventDefault();
      gridRef.current?.focus({ preventScroll: true });
      openRowMenu(i, r, e.clientX, e.clientY, false);
    },
    pointerDown: (e, r) => {
      // Rows drag onto playlists in the sidebar: the selection, or this row.
      if ((e.target as HTMLElement).closest('button, input, a')) return;
      beginDrag(e, () => {
        const ids = selectionRef.current.has(r.id) ? [...selectionRef.current] : [r.id];
        return { kind: 'tracks', ids, label: ids.length === 1 ? r.displayTitle : plural(ids.length, 'track') };
      });
    },
  };
  useLayoutEffect(() => {
    handlers.current = latestHandlers;
  });
  const rowEvents = useMemo<RowEvents>(
    () => ({
      click: (e, i, r) => handlers.current?.click(e, i, r),
      toggle: (i, r) => handlers.current?.toggle(i, r),
      open: (r) => handlers.current?.open(r),
      menu: (e, i, r) => handlers.current?.menu(e, i, r),
      pointerDown: (e, r) => handlers.current?.pointerDown(e, r),
    }),
    [],
  );

  const activeId = activeRow?.id ?? null;
  const rows: ReactNode[] = [];
  let activeElementId: string | undefined;
  if (count > 0) {
    for (let i = first; i <= last; i += 1) {
      const r = rowAt(i);
      const top = i * ROW;
      // Keyed by position: during a reload a moved track can briefly appear in two pages.
      if (!r) {
        rows.push(<div key={i} className="trow is-placeholder" style={{ top, gridTemplateColumns: template }} aria-hidden />);
        continue;
      }
      const selected = selection.has(r.id);
      const isActive = r.id === activeId;
      const id = `${gridId}-r${i}`;
      if (isActive && (activeElementId === undefined || i === active)) activeElementId = id;
      rows.push(
        <TableRow
          key={i}
          index={i}
          row={r}
          id={id}
          selected={selected}
          active={isActive}
          playing={r.id === playingId}
          template={template}
          columns={columns}
          ctx={cellCtx}
          events={rowEvents}
        />,
      );
    }
  }

  return (
    <>
      <div
        ref={gridRef}
        className="ttable"
        role="grid"
        aria-label={label}
        aria-rowcount={count + 1}
        aria-colcount={columns.length + 1}
        aria-multiselectable
        tabIndex={0}
        aria-activedescendant={activeElementId}
        onKeyDown={onKeyDown}
        onFocus={onFocus}
      >
        <div className="thead" role="rowgroup">
          <div className="trow trow-head" role="row" aria-rowindex={1} style={{ gridTemplateColumns: template }}>
            <div role="columnheader" className="tcell tcell-check">
              <Checkbox
                checked={allSelected}
                indeterminate={someSelected}
                aria-label={allSelected ? 'Clear selection' : 'Select all tracks'}
                disabled={count === 0}
                onChange={() => {
                  if (allSelected || someSelected) onSelection(new Set());
                  else selectAll();
                }}
              />
            </div>
            {columns.map((c) => {
              const sorted = c.sortKey && sort.key === c.sortKey;
              return (
                <div
                  key={c.id}
                  role="columnheader"
                  aria-sort={sorted ? (sort.desc ? 'descending' : 'ascending') : undefined}
                  className={`tcell th ${c.align === 'end' ? 'is-end' : ''}`}
                >
                  {c.sortKey ? (
                    <button type="button" className={`th-btn ${sorted ? 'is-sorted' : ''}`} onClick={() => headerClick(c)} data-tip={c.description}>
                      <span>{c.label}</span>
                      {sorted ? sort.desc ? <LuArrowDown aria-hidden className="th-arrow" /> : <LuArrowUp aria-hidden className="th-arrow" /> : null}
                    </button>
                  ) : (
                    <span className="th-static">{c.label}</span>
                  )}
                </div>
              );
            })}
          </div>
        </div>
        <div ref={scrollRef} className="tbody" role="rowgroup" onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
          {loadError && total == null ? (
            <div className="ttable-empty">
              <ErrorState title="The tracks could not be loaded" error={loadError} onRetry={retry} />
            </div>
          ) : total === 0 ? (
            <div className="ttable-empty">{empty}</div>
          ) : (
            <div className="tbody-inner" style={{ height: count * ROW }}>
              {rows}
            </div>
          )}
        </div>
        {loadError && total != null ? (
          <div className="ttable-error" role="alert">
            <span>Some rows could not be loaded: {loadError}</span>
            <button type="button" className="link" onClick={retry}>
              Try again
            </button>
          </div>
        ) : null}
      </div>
      <ContextMenu request={menu} onClose={closeMenu} />
    </>
  );
}
