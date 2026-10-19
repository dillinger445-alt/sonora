/**
 * Dropdown menus: action menus (close on select) and checkbox menus for
 * filters (stay open). Full keyboard support: arrows, Home/End, Enter or
 * Space to activate, Escape to close and return focus to the trigger.
 */

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ComponentType, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { LuCheck } from 'react-icons/lu';

type IconComponent = ComponentType<{ className?: string; 'aria-hidden'?: boolean }>;

export interface MenuAction {
  kind?: 'action';
  id: string;
  label: string;
  icon?: IconComponent;
  hint?: string;
  disabled?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

export interface MenuCheck {
  kind: 'check';
  id: string;
  label: string;
  checked: boolean;
  count?: number;
  onToggle: () => void;
}

export interface MenuSeparator {
  kind: 'separator';
  id: string;
}

export type MenuItem = MenuAction | MenuCheck | MenuSeparator;

export interface MenuProps {
  /** Renders the trigger; spread the props onto a button. */
  trigger: (props: {
    ref: (el: HTMLButtonElement | null) => void;
    onClick: () => void;
    onKeyDown: (e: KeyboardEvent) => void;
    'aria-haspopup': 'menu';
    'aria-expanded': boolean;
    'aria-controls': string;
  }) => ReactNode;
  items: MenuItem[];
  align?: 'start' | 'end';
  label: string;
  footer?: ReactNode;
}

export function Menu({ trigger, items, align = 'start', label, footer }: MenuProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number; minWidth: number; maxHeight: number } | null>(null);
  const [active, setActive] = useState(-1);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const focusable = items.map((it, i) => ({ it, i })).filter(({ it }) => it.kind !== 'separator' && !(it.kind !== 'check' && it.disabled));

  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    setActive(-1);
    if (returnFocus) triggerRef.current?.focus();
  }, []);

  const place = useCallback(() => {
    const t = triggerRef.current;
    const m = menuRef.current;
    if (!t) return;
    const r = t.getBoundingClientRect();
    const mw = m?.offsetWidth ?? 220;
    const mh = m?.scrollHeight ?? 200;
    const spaceBelow = window.innerHeight - r.bottom - 12;
    const spaceAbove = r.top - 12;
    const below = spaceBelow >= Math.min(mh, 240) || spaceBelow >= spaceAbove;
    const maxHeight = Math.max(120, below ? spaceBelow : spaceAbove);
    let left = align === 'end' ? r.right - mw : r.left;
    left = Math.max(8, Math.min(left, window.innerWidth - mw - 8));
    const top = below ? r.bottom + 4 : Math.max(8, r.top - 4 - Math.min(mh, maxHeight));
    setPos({ left, top, minWidth: r.width, maxHeight });
  }, [align]);

  useLayoutEffect(() => {
    if (open) place();
  }, [open, place, items.length]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (menuRef.current?.contains(t) || triggerRef.current?.contains(t)) return;
      close(false);
    };
    const onResize = () => close(false);
    document.addEventListener('pointerdown', onDown, true);
    window.addEventListener('resize', onResize);
    return () => {
      document.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('resize', onResize);
    };
  }, [open, close]);

  // Runs again once the menu is positioned: it is hidden (and cannot take
  // focus) until then.
  useEffect(() => {
    if (!open || active < 0 || !pos) return;
    menuRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.focus();
  }, [open, active, pos]);

  const openMenu = (focusFirst: boolean) => {
    setOpen(true);
    setActive(focusFirst ? (focusable[0]?.i ?? -1) : -1);
    if (!focusFirst) requestAnimationFrame(() => menuRef.current?.focus());
  };

  const move = (dir: 1 | -1 | 'first' | 'last') => {
    if (focusable.length === 0) return;
    const idx = focusable.findIndex((f) => f.i === active);
    let next: number;
    if (dir === 'first') next = 0;
    else if (dir === 'last') next = focusable.length - 1;
    else next = (idx + dir + focusable.length) % focusable.length;
    setActive(focusable[next]?.i ?? -1);
  };

  const activate = (item: MenuItem) => {
    if (item.kind === 'separator') return;
    if (item.kind === 'check') {
      item.onToggle();
      return;
    }
    if (item.disabled) return;
    close(true);
    item.onSelect();
  };

  const onMenuKey = (e: KeyboardEvent) => {
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
        e.preventDefault();
        e.stopPropagation();
        close(true);
        break;
      case 'Tab':
        // The menu sits right after its trigger in the Tab order: Shift+Tab
        // lands on the trigger, and Tab continues from it (rather than from
        // the end of the page, where the menu is rendered).
        close(true);
        if (e.shiftKey) e.preventDefault();
        break;
      case 'Enter':
      case ' ': {
        const item = items[active];
        if (item) {
          e.preventDefault();
          activate(item);
        }
        break;
      }
      default:
        break;
    }
  };

  return (
    <>
      {trigger({
        ref: (el) => {
          triggerRef.current = el;
        },
        onClick: () => (open ? close(false) : openMenu(false)),
        onKeyDown: (e) => {
          if (open && e.key === 'Escape') {
            e.preventDefault();
            close(true);
          } else if (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            openMenu(true);
          }
        },
        'aria-haspopup': 'menu',
        'aria-expanded': open,
        'aria-controls': menuId,
      })}
      {open
        ? createPortal(
            <div
              ref={menuRef}
              id={menuId}
              role="menu"
              aria-label={label}
              tabIndex={-1}
              className="menu"
              style={pos ? { left: pos.left, top: pos.top, minWidth: Math.max(180, pos.minWidth), maxHeight: pos.maxHeight } : { visibility: 'hidden' }}
              onKeyDown={onMenuKey}
            >
              {items.map((item, i) => {
                if (item.kind === 'separator') return <div key={item.id} className="menu-sep" role="separator" />;
                if (item.kind === 'check') {
                  return (
                    <div
                      key={item.id}
                      role="menuitemcheckbox"
                      aria-checked={item.checked}
                      tabIndex={-1}
                      data-index={i}
                      className={`menu-item menu-check ${item.checked ? 'is-checked' : ''}`}
                      onClick={() => {
                        setActive(i);
                        activate(item);
                      }}
                    >
                      <span className="menu-checkbox" aria-hidden>
                        {item.checked ? <LuCheck /> : null}
                      </span>
                      <span className="menu-label">{item.label}</span>
                      {item.count != null ? <span className="menu-count num">{item.count}</span> : null}
                    </div>
                  );
                }
                const Icon = item.icon;
                return (
                  <div
                    key={item.id}
                    role="menuitem"
                    tabIndex={-1}
                    data-index={i}
                    aria-disabled={item.disabled || undefined}
                    className={`menu-item ${item.danger ? 'is-danger' : ''} ${item.disabled ? 'is-disabled' : ''}`}
                    onClick={() => activate(item)}
                  >
                    {Icon ? <Icon className="menu-icon" aria-hidden /> : <span className="menu-icon" />}
                    <span className="menu-label">{item.label}</span>
                    {item.hint ? <span className="menu-hint">{item.hint}</span> : null}
                  </div>
                );
              })}
              {footer ? <div className="menu-footer">{footer}</div> : null}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
