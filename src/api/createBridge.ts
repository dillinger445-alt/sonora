import type { Bridge } from './bridge.ts';

/** The desktop app talks to the core over Tauri IPC; a browser uses the dev bridge. */
export async function createBridge(): Promise<Bridge> {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    const { createTauriBridge } = await import('./tauriBridge.ts');
    return createTauriBridge();
  }
  const { createHttpBridge } = await import('./httpBridge.ts');
  return createHttpBridge();
}
