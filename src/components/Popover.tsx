/**
 * A panel of controls anchored to a button (filters that need more than a
 * checkbox list). Not modal: Escape, a click outside or tabbing past either
 * end closes it, and focus returns to the trigger when the keyboard closed it.
 */

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { focusIsLost } from '../lib/focus.ts';

const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface PopoverTriggerProps {
  ref: (el: HTMLButtonElement | null) => void;
  onClick: () => void;
  onKeyDown: (e: KeyboardEvent) => void;
  'aria-haspopup': 'dialog';
  'aria-expanded': boolean;
  'aria-controls': string;
}

export interface PopoverProps {
  trigger: (props: PopoverTriggerProps) => ReactNode;
  /** The panel's content; `close` closes it (returning focus to the trigger). */
  children: (close: () => void) => ReactNode;
  label: string;
  align?: 'start' | 'end';
  width?: number;
}

export function Popover({ trigger, children, label, align = 'start', width = 300 }: PopoverProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const id = useId();

  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    setPos(null);
    if (returnFocus) triggerRef.current?.focus();
  }, []);

  const place = useCallback(() => {
    const t = triggerRef.current;
    if (!t) return;
    const r = t.getBoundingClientRect();
    const h = panelRef.current?.scrollHeight ?? 320;
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const down = below >= Math.min(h, 360) || below >= above;
    const maxHeight = Math.max(160, down ? below : above);
    let left = align === 'end' ? r.right - width : r.left;
    left = Math.max(8, Math.min(left, window.innerWidth - width - 8));
    const top = down ? r.bottom + 4 : Math.max(8, r.top - 4 - Math.min(h, maxHeight));
    setPos({ left, top, maxHeight });
  }, [align, width]);

  useLayoutEffect(() => {
    if (open) place();
  }, [open, place]);

  // Focus the first control once the panel is placed (it is hidden before).
  const focused = useRef(false);
  useEffect(() => {
    if (!open) {
      focused.current = false;
      return;
    }
    if (!pos || focused.current) return;
    focused.current = true;
    const p = panelRef.current;
    (p?.querySelector<HTMLElement>('[data-autofocus]') ?? p?.querySelector<HTMLElement>(FOCUSABLE) ?? p)?.focus();
  }, [open, pos]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (panelRef.current?.contains(t) || triggerRef.current?.contains(t)) return;
      close(false);
    };
    const onResize = () => close(false);
    // Escape also closes it when focus fell back to the page (a control
    // inside was disabled, say).
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented && focusIsLost()) {
        e.preventDefault();
        close(true);
      }
    };
    document.addEventListener('pointerdown', onDown, true);
    document.addEventListener('keydown', onKey);
    window.addEventListener('resize', onResize);
    return () => {
      document.removeEventListener('pointerdown', onDown, true);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', onResize);
    };
  }, [open, close]);

  const onPanelKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
      return;
    }
    if (e.key !== 'Tab') return;
    const items = [...(panelRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
    const first = items[0];
    const last = items[items.length - 1];
    if ((e.shiftKey && document.activeElement === first) || (!e.shiftKey && document.activeElement === last)) {
      e.preventDefault();
      close(true);
    }
  };

  return (
    <>
      {trigger({
        ref: (el) => {
          triggerRef.current = el;
        },
        onClick: () => (open ? close(false) : setOpen(true)),
        onKeyDown: (e) => {
          if (open && e.key === 'Escape') {
            e.preventDefault();
            close(true);
          } else if (!open && e.key === 'ArrowDown') {
            e.preventDefault();
            setOpen(true);
          }
        },
        'aria-haspopup': 'dialog',
        'aria-expanded': open,
        'aria-controls': id,
      })}
      {open
        ? createPortal(
            <div
              ref={panelRef}
              id={id}
              role="dialog"
              aria-label={label}
              tabIndex={-1}
              className="popover"
              style={pos ? { left: pos.left, top: pos.top, width, maxHeight: pos.maxHeight } : { visibility: 'hidden', width }}
              onKeyDown={onPanelKey}
            >
              {children(() => close(true))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
