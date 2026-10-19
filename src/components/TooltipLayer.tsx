/**
 * One tooltip for the whole app: any element with `data-tip` gets a
 * tooltip on hover (after a short delay) and on keyboard focus.
 * The element's accessible name stays its own; the tooltip only adds a
 * visible hint (icon buttons carry `aria-label` with the same text).
 */

import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

interface Tip {
  text: string;
  x: number;
  y: number;
  below: boolean;
}

const DELAY_MS = 450;

export function TooltipLayer() {
  const [tip, setTip] = useState<Tip | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const current = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const clear = () => {
      if (timer.current) clearTimeout(timer.current);
      timer.current = null;
    };
    const hide = () => {
      clear();
      current.current = null;
      setTip(null);
    };
    const show = (el: HTMLElement, immediate: boolean) => {
      const text = el.dataset.tip;
      if (!text) return;
      clear();
      current.current = el;
      const place = () => {
        if (current.current !== el || !el.isConnected) return;
        const r = el.getBoundingClientRect();
        const below = r.top < 48;
        setTip({ text, x: r.left + r.width / 2, y: below ? r.bottom + 6 : r.top - 6, below });
      };
      if (immediate) place();
      else timer.current = setTimeout(place, DELAY_MS);
    };
    const target = (e: Event) => (e.target instanceof Element ? (e.target.closest('[data-tip]') as HTMLElement | null) : null);
    const over = (e: PointerEvent) => {
      const el = target(e);
      if (el === current.current) return;
      if (!el) {
        hide();
        return;
      }
      show(el, false);
    };
    const focusIn = (e: FocusEvent) => {
      const el = target(e);
      if (el && el.matches(':focus-visible')) show(el, true);
    };
    const down = () => hide();
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') hide();
    };
    document.addEventListener('pointerover', over);
    document.addEventListener('focusin', focusIn);
    document.addEventListener('focusout', hide);
    document.addEventListener('pointerdown', down, true);
    document.addEventListener('keydown', key, true);
    window.addEventListener('scroll', hide, true);
    window.addEventListener('blur', hide);
    return () => {
      clear();
      document.removeEventListener('pointerover', over);
      document.removeEventListener('focusin', focusIn);
      document.removeEventListener('focusout', hide);
      document.removeEventListener('pointerdown', down, true);
      document.removeEventListener('keydown', key, true);
      window.removeEventListener('scroll', hide, true);
      window.removeEventListener('blur', hide);
    };
  }, []);

  if (!tip) return null;
  const left = Math.min(Math.max(tip.x, 12), window.innerWidth - 12);
  return createPortal(
    <div
      className={`tooltip ${tip.below ? 'tooltip-below' : ''}`}
      style={{ left, top: tip.y }}
      role="tooltip"
      // Keep tooltips inside the window horizontally.
      ref={(el) => {
        if (!el) return;
        const r = el.getBoundingClientRect();
        const overflowRight = r.right - (window.innerWidth - 8);
        const overflowLeft = 8 - r.left;
        if (overflowRight > 0) el.style.transform = `translate(calc(-50% - ${overflowRight}px), ${tip.below ? '0' : '-100%'})`;
        else if (overflowLeft > 0) el.style.transform = `translate(calc(-50% + ${overflowLeft}px), ${tip.below ? '0' : '-100%'})`;
      }}
    >
      {tip.text}
    </div>,
    document.body,
  );
}
