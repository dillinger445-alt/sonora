/** Canvas helpers: crisp drawing at any device pixel ratio, theme colours. */

import { useEffect, useState } from 'react';

/**
 * The display's pixel ratio, updated when it changes (the window moves to
 * another screen, or the page is zoomed), so canvases can redraw sharply.
 */
export function useDevicePixelRatio(): number {
  const [dpr, setDpr] = useState(() => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1));
  useEffect(() => {
    let query: MediaQueryList | null = null;
    const listen = () => {
      query?.removeEventListener('change', listen);
      const now = window.devicePixelRatio || 1;
      setDpr(now);
      query = window.matchMedia(`(resolution: ${now}dppx)`);
      query.addEventListener('change', listen);
    };
    listen();
    return () => query?.removeEventListener('change', listen);
  }, []);
  return dpr;
}

export function setupCanvas(canvas: HTMLCanvasElement, cssWidth: number, cssHeight: number): CanvasRenderingContext2D | null {
  const dpr = Math.max(1, Math.min(3, window.devicePixelRatio || 1));
  const w = Math.max(1, Math.round(cssWidth * dpr));
  const h = Math.max(1, Math.round(cssHeight * dpr));
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  canvas.style.width = `${cssWidth}px`;
  canvas.style.height = `${cssHeight}px`;
  const ctx = canvas.getContext('2d');
  if (!ctx) return null;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);
  return ctx;
}

/** Reads a CSS custom property from :root (falls back when unavailable). */
export function cssVar(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

export interface Theme {
  text: string;
  text2: string;
  text3: string;
  grid: string;
  line: string;
  lime: string;
  amber: string;
  red: string;
  low: string;
  mid: string;
  high: string;
  envelope: string;
  well: string;
  data1: string;
  data2: string;
}

export function theme(): Theme {
  return {
    text: cssVar('--c-text', '#e3e6e9'),
    text2: cssVar('--c-text-2', '#a3abb3'),
    text3: cssVar('--c-text-3', '#8c949d'),
    grid: cssVar('--c-grid', '#2a2f35'),
    line: cssVar('--c-line', '#31363c'),
    lime: cssVar('--c-lime', '#c0e84e'),
    amber: cssVar('--c-amber', '#e9ae40'),
    red: cssVar('--c-red', '#f06a5e'),
    low: cssVar('--c-band-low', '#4e6d8c'),
    mid: cssVar('--c-band-mid', '#8ea4b9'),
    high: cssVar('--c-band-high', '#dce4ea'),
    envelope: '#3b424a',
    well: cssVar('--c-well', '#141619'),
    data1: cssVar('--c-data-1', '#9fb7cc'),
    data2: cssVar('--c-data-2', '#6f8aa3'),
  };
}

export const MONO_FONT = "500 10px 'Martian Mono', ui-monospace, monospace";
export const SANS_FONT = "500 11px 'Archivo', ui-sans-serif, system-ui, sans-serif";
