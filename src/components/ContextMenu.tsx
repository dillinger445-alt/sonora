/**
 * A menu opened where the pointer is (right-click), or at a row from the
 * keyboard (Shift+F10 or the Menu key). It looks and works like `Menu`:
 * arrows move, Enter or Space chooses, Escape closes and puts focus back
 * where it was.
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import type { MenuAction, MenuSeparator } from './Menu.tsx';

export interface ContextMenuRequest {
  /** Where it opens, in window coordinates. */
  x: number;
  y: number;
  label: string;
  items: (MenuAction | MenuSeparator)[];
  /** Opened from the keyboard: the first item takes focus. */
  keyboard?: boolean;
  /** Where focus goes back to when the menu closes. */
  restoreFocus?: HTMLElement | null;
}

export function ContextMenu({ request, onClose }: { request: ContextMenuRequest | null; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  const [active, setActive] = useState(-1);
  const items = request?.items ?? [];
  const focusable = items.flatMap((it, i) => (it.kind === 'separator' || it.disabled ? [] : [i]));

  const close = useCallback(
    (restore: boolean) => {
      const back = request?.restoreFocus;
      onClose();
      if (restore && back?.isConnected) back.focus({ preventScroll: true });
    },
    [request, onClose],
  );

  // Inside the window: it opens below and to the right of the point when
  // there is room, otherwise above or to the left.
  useLayoutEffect(() => {
    setPos(null);
    setActive(-1);
    if (!request) return;
    const m = ref.current;
    const w = m?.offsetWidth ?? 220;
    const h = m?.scrollHeight ?? 240;
    const maxHeight = window.innerHeight - 16;
    const left = request.x + w + 8 <= window.innerWidth ? request.x : Math.max(8, request.x - w);
    const top = request.y + Math.min(h, maxHeight) + 8 <= window.innerHeight ? request.y : Math.max(8, request.y - Math.min(h, maxHeight));
    setPos({ left, top, maxHeight });
    if (request.keyboard) setActive(request.items.findIndex((it) => it.kind !== 'separator' && !it.disabled));
  }, [request]);

  // Focus follows the active item once the menu is placed (hidden until then).
  useEffect(() => {
    if (!request || !pos) return;
    if (active >= 0) ref.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.focus();
    else ref.current?.focus();
  }, [request, pos, active]);

  useEffect(() => {
    if (!request) return;
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) close(false);
    };
    const away = () => close(false);
    // Scrolling what is under it would leave it pointing at another row
    // (the menu's own scrolling is fine).
    const onScroll = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) close(false);
    };
    document.addEventListener('pointerdown', onDown, true);
    window.addEventListener('resize', away);
    window.addEventListener('blur', away);
    document.addEventListener('scroll', onScroll, true);
    return () => {
      document.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('resize', away);
      window.removeEventListener('blur', away);
      document.removeEventListener('scroll', onScroll, true);
    };
  }, [request, close]);

  if (!request) return null;

  const choose = (item: MenuAction) => {
    if (item.disabled) return;
    close(true);
    item.onSelect();
  };

  const move = (to: 1 | -1 | 'first' | 'last') => {
    if (!focusable.length) return;
    const at = focusable.indexOf(active);
    const next =
      to === 'first' ? 0 : to === 'last' ? focusable.length - 1 : at < 0 ? (to === 1 ? 0 : focusable.length - 1) : (at + to + focusable.length) % focusable.length;
    setActive(focusable[next] ?? -1);
  };

  const onKey = (e: KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(-1);
        break;
      case 'Home':
        e.preventDefault();
        move('first');
        break;
      case 'End':
        e.preventDefault();
        move('last');
        break;
      case 'Escape':
      case 'Tab':
        e.preventDefault();
        e.stopPropagation();
        close(true);
        break;
      case 'Enter':
      case ' ': {
        const item = items[active];
        if (item && item.kind !== 'separator') {
          e.preventDefault();
          choose(item);
        }
        break;
      }
      default:
        break;
    }
  };

  return createPortal(
    <div
      ref={ref}
      role="menu"
      aria-label={request.label}
      tabIndex={-1}
      className="menu"
      style={pos ? { left: pos.left, top: pos.top, minWidth: 200, maxHeight: pos.maxHeight } : { left: 0, top: 0, visibility: 'hidden' }}
      onKeyDown={onKey}
      // A second right-click inside the menu does not open the web view's own.
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) => {
        if (item.kind === 'separator') return <div key={item.id} className="menu-sep" role="separator" />;
        const Icon = item.icon;
        return (
          <div
            key={item.id}
            role="menuitem"
            tabIndex={-1}
            data-index={i}
            aria-disabled={item.disabled || undefined}
            className={`menu-item ${item.danger ? 'is-danger' : ''} ${item.disabled ? 'is-disabled' : ''}`}
            onClick={() => choose(item)}
            onPointerMove={() => {
              if (!item.disabled && active !== i) setActive(i);
            }}
          >
            {Icon ? <Icon className="menu-icon" aria-hidden /> : <span className="menu-icon" />}
            <span className="menu-label">{item.label}</span>
            {item.hint ? <span className="menu-hint">{item.hint}</span> : null}
          </div>
        );
      })}
    </div>,
    document.body,
  );
}
