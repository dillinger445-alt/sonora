/**
 * Dragging inside the window, with pointer events: tracks onto playlists,
 * tracks within a playlist, playlists within the tree.
 *
 * HTML5 drag and drop is not used: the desktop window takes file drops
 * from the system (Tauri `dragDropEnabled`), which on Windows stops the
 * web view's own drag events.
 *
 * A drag starts once the pointer has moved a few pixels with the button
 * down, so clicks still click. Escape cancels. Drop targets are elements
 * with `data-drop="<id>"` whose handlers are registered with
 * `useDropTarget`; scrollable areas marked `data-autoscroll` scroll when
 * the pointer nears their edges.
 */

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { LuListMusic, LuMusic } from 'react-icons/lu';

export type DragPayload =
  | { kind: 'tracks'; ids: number[]; label: string; from?: { playlistId: number } }
  | { kind: 'node'; id: number; label: string };

/** Where over a target a drop would land (the target decides what it means). */
export interface DropHint {
  where: 'before' | 'after' | 'inside';
  /** For lists: the index the dragged items would go to. */
  index?: number;
  /** Copy (add to a playlist) or move. */
  effect?: 'copy' | 'move';
}

export interface DropTarget {
  /**
   * The hint for a pointer at (x, y) over the target element `el` (the
   * innermost element under the pointer is `hit`), or `null` when it
   * cannot drop there.
   */
  over: (p: DragPayload, at: { x: number; y: number; el: HTMLElement; hit: HTMLElement }) => DropHint | null;
  drop: (p: DragPayload, hint: DropHint) => void;
}

export interface DragState {
  payload: DragPayload;
  target: string | null;
  hint: DropHint | null;
}

interface DndContextValue {
  register: (id: string, t: DropTarget) => () => void;
  /** Call from onPointerDown; `payload` is read once the drag really starts. */
  begin: (e: ReactPointerEvent, payload: () => DragPayload | null) => void;
  state: DragState | null;
}

const DndContext = createContext<DndContextValue | null>(null);

function useDnd(): DndContextValue {
  const v = useContext(DndContext);
  if (!v) throw new Error('useDnd outside DndProvider');
  return v;
}

/** The drag in progress (re-renders while it moves between targets). */
export function useDragState(): DragState | null {
  return useDnd().state;
}

export function useDragStart(): DndContextValue['begin'] {
  return useDnd().begin;
}

/** Registers the handlers for elements with `data-drop={id}`. */
export function useDropTarget(id: string, target: DropTarget): void {
  const { register } = useDnd();
  const ref = useRef(target);
  useLayoutEffect(() => {
    ref.current = target;
  });
  useEffect(
    () =>
      register(id, {
        over: (p, at) => ref.current.over(p, at),
        drop: (p, h) => ref.current.drop(p, h),
      }),
    [id, register],
  );
}

const THRESHOLD = 5;
const EDGE = 36;

export function DndProvider({ children }: { children: ReactNode }) {
  const targets = useRef(new Map<string, DropTarget>());
  const [state, setState] = useState<DragState | null>(null);
  const [ghost, setGhost] = useState<{ x: number; y: number } | null>(null);
  const live = useRef<DragState | null>(null);
  const pointer = useRef({ x: 0, y: 0 });

  const register = useCallback((id: string, t: DropTarget) => {
    targets.current.set(id, t);
    return () => {
      if (targets.current.get(id) === t) targets.current.delete(id);
    };
  }, []);

  const update = useCallback((next: DragState | null) => {
    const prev = live.current;
    live.current = next;
    // Re-render only when the target or the hint changes.
    if (
      !prev ||
      !next ||
      prev.target !== next.target ||
      prev.hint?.where !== next.hint?.where ||
      prev.hint?.index !== next.hint?.index ||
      prev.hint?.effect !== next.hint?.effect
    ) {
      setState(next);
    }
  }, []);

  const hitTest = useCallback((payload: DragPayload, x: number, y: number): { target: string | null; hint: DropHint | null } => {
    const hit = document.elementFromPoint(x, y) as HTMLElement | null;
    const el = hit?.closest<HTMLElement>('[data-drop]');
    const id = el?.dataset.drop;
    const t = id ? targets.current.get(id) : undefined;
    if (!hit || !el || !id || !t) return { target: null, hint: null };
    const hint = t.over(payload, { x, y, el, hit });
    return hint ? { target: id, hint } : { target: null, hint: null };
  }, []);

  const begin = useCallback(
    (e: ReactPointerEvent, payloadFn: () => DragPayload | null) => {
      if (e.button !== 0 || e.pointerType === 'touch') return;
      const start = { x: e.clientX, y: e.clientY };
      let started = false;
      let raf = 0;
      let payload: DragPayload | null = null;

      const autoscroll = () => {
        const { x, y } = pointer.current;
        const area = (document.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>('[data-autoscroll]');
        if (area) {
          const r = area.getBoundingClientRect();
          const step = y < r.top + EDGE ? -Math.ceil((r.top + EDGE - y) / 4) : y > r.bottom - EDGE ? Math.ceil((y - (r.bottom - EDGE)) / 4) : 0;
          if (step !== 0) {
            area.scrollTop += step;
            if (payload) {
              const h = hitTest(payload, x, y);
              update({ payload, ...h });
            }
          }
        }
        raf = requestAnimationFrame(autoscroll);
      };

      const move = (ev: PointerEvent) => {
        pointer.current = { x: ev.clientX, y: ev.clientY };
        if (!started) {
          if (Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < THRESHOLD) return;
          payload = payloadFn();
          if (!payload) {
            stop();
            return;
          }
          started = true;
          document.body.classList.add('is-dragging');
          window.getSelection()?.removeAllRanges();
          raf = requestAnimationFrame(autoscroll);
        }
        if (!payload) return;
        ev.preventDefault();
        setGhost({ x: ev.clientX, y: ev.clientY });
        update({ payload, ...hitTest(payload, ev.clientX, ev.clientY) });
      };

      const up = (ev: PointerEvent) => {
        const s = live.current;
        stop();
        if (!started || !s) return;
        // The click that ends a drag is not a click.
        const swallow = (c: MouseEvent) => {
          c.stopPropagation();
          c.preventDefault();
        };
        window.addEventListener('click', swallow, { capture: true, once: true });
        setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);
        const final = hitTest(s.payload, ev.clientX, ev.clientY);
        const target = final.target ? targets.current.get(final.target) : undefined;
        if (target && final.hint) target.drop(s.payload, final.hint);
      };

      const key = (ev: KeyboardEvent) => {
        if (ev.key !== 'Escape') return;
        // Escape cancels a drag, and only that (not the selection too).
        if (started) {
          ev.preventDefault();
          ev.stopPropagation();
        }
        stop();
      };

      function stop() {
        window.removeEventListener('pointermove', move);
        window.removeEventListener('pointerup', up);
        window.removeEventListener('pointercancel', cancel);
        window.removeEventListener('keydown', key, true);
        cancelAnimationFrame(raf);
        document.body.classList.remove('is-dragging');
        setGhost(null);
        update(null);
      }
      const cancel = () => stop();

      window.addEventListener('pointermove', move);
      window.addEventListener('pointerup', up);
      window.addEventListener('pointercancel', cancel);
      window.addEventListener('keydown', key, true);
    },
    [hitTest, update],
  );

  const value = useMemo<DndContextValue>(() => ({ register, begin, state }), [register, begin, state]);
  const payload = state?.payload;
  const effect = state?.hint?.effect;
  return (
    <DndContext.Provider value={value}>
      {children}
      {payload && ghost
        ? createPortal(
            <div className={`drag-ghost ${state?.target ? 'is-over' : ''}`} style={{ transform: `translate(${ghost.x + 14}px, ${ghost.y + 12}px)` }} aria-hidden>
              {payload.kind === 'tracks' ? <LuMusic /> : <LuListMusic />}
              <span className="drag-ghost-label">{payload.label}</span>
              {state?.target && effect === 'copy' ? <span className="drag-ghost-plus">+</span> : null}
            </div>,
            document.body,
          )
        : null}
    </DndContext.Provider>
  );
}
