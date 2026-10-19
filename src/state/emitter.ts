/** Minimal synchronous event emitter for core events inside the interface. */

export type Listener = (name: string, payload: unknown) => void;

export class Emitter {
  private listeners = new Set<Listener>();

  on(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  emit(name: string, payload: unknown): void {
    for (const l of [...this.listeners]) {
      try {
        l(name, payload);
      } catch (err) {
        console.error('event listener failed', name, err);
      }
    }
  }
}

/**
 * Calls `fn` at most once per `ms`, always delivering the latest call
 * (leading and trailing edge).
 */
export function throttle(fn: () => void, ms: number): { run: () => void; cancel: () => void } {
  let last = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const run = () => {
    const now = Date.now();
    const wait = last + ms - now;
    if (wait <= 0) {
      last = now;
      fn();
    } else if (!timer) {
      timer = setTimeout(() => {
        timer = null;
        last = Date.now();
        fn();
      }, wait);
    }
  };
  const cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };
  return { run, cancel };
}
