/**
 * Loads data from the core and reloads it when given core events fire.
 * Keeps showing the previous data while a reload is in flight, so lists
 * do not flicker during scans.
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { throttle } from './emitter.ts';
import { useCoreEvent } from './core.tsx';

export interface Resource<T> {
  data: T | null;
  error: Error | null;
  /** True until the first load finishes. */
  loading: boolean;
  reload: () => void;
}

export interface ResourceOptions {
  /** Core events that make the data stale. */
  events?: string[];
  /** Minimum time between event-driven reloads. */
  throttleMs?: number;
  enabled?: boolean;
}

export function useResource<T>(fetcher: () => Promise<T>, deps: readonly unknown[], opts: ResourceOptions = {}): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);
  const fetchRef = useRef(fetcher);
  useLayoutEffect(() => {
    fetchRef.current = fetcher;
  });
  const enabled = opts.enabled ?? true;

  const load = useCallback(() => {
    const id = ++seq.current;
    fetchRef.current().then(
      (d) => {
        if (id !== seq.current) return;
        setData(d);
        setError(null);
        setLoading(false);
      },
      (e: unknown) => {
        if (id !== seq.current) return;
        setError(e instanceof Error ? e : new Error(String(e)));
        setLoading(false);
      },
    );
  }, []);

  useEffect(() => {
    if (!enabled) return;
    setLoading(true);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, load, ...deps]);

  const t = useMemo(() => throttle(load, opts.throttleMs ?? 1000), [load, opts.throttleMs]);
  useEffect(() => () => t.cancel(), [t]);
  useCoreEvent(opts.events ?? [], () => {
    if (enabled) t.run();
  });

  return { data, error, loading, reload: load };
}
