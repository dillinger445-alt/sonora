/**
 * Per-window interface preferences (sort order, open panels) kept in
 * localStorage. Purely a convenience: everything works without it.
 */

import { useCallback, useEffect, useState } from 'react';

const PREFIX = 'sonora.ui.';

export function readPref<T>(key: string, fallback: T, valid?: (v: unknown) => v is T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    if (raw == null) return fallback;
    const v: unknown = JSON.parse(raw);
    if (valid && !valid(v)) return fallback;
    return v as T;
  } catch {
    return fallback;
  }
}

export function writePref(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage may be unavailable (private mode); preferences then last for the session.
  }
}

export function usePref<T>(key: string, fallback: T, valid?: (v: unknown) => v is T): [T, (v: T) => void] {
  const [value, setValue] = useState<T>(() => readPref(key, fallback, valid));
  const set = useCallback(
    (v: T) => {
      setValue(v);
      writePref(key, v);
    },
    [key],
  );
  return [value, set];
}

/** The value, once it has stopped changing for `ms` (e.g. the row under a moving cursor). */
export function useDebouncedValue<T>(value: T, ms: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return settled;
}

/** Debounces a fast-changing value (e.g. search input). */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number): ((...args: A) => void) & { cancel: () => void } {
  let t: ReturnType<typeof setTimeout> | null = null;
  const run = (...args: A) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => {
      t = null;
      fn(...args);
    }, ms);
  };
  run.cancel = () => {
    if (t) clearTimeout(t);
    t = null;
  };
  return run;
}
