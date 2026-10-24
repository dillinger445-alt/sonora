/**
 * Browser bridge: talks to `sonora-devserver` (the same Rust core) over
 * HTTP + Server-Sent Events. Used for browser development and end-to-end
 * tests; the desktop app uses `tauriBridge.ts`.
 *
 * Browsers cannot hand out absolute file paths, so native pickers and
 * drag-and-drop with paths are unavailable here (`native: false`); the
 * interface asks for paths instead.
 */

import { CoreError, EVENTS_RECONNECTED, type Bridge, type DropHandlers, type Updater, type UpdateStep } from './bridge.ts';
import type { CoreEventName, PreparedPlayback } from './types.ts';

/**
 * Every core event, listened for by name (Server-Sent Events have no
 * catch-all). The type check fails when an event is missing here.
 */
const EVENT_NAMES = Object.keys({
  'scan.started': true,
  'scan.progress': true,
  'scan.item': true,
  'scan.finished': true,
  'library.indexed': true,
  'library.changed': true,
  'duplicates.progress': true,
  'duplicates.updated': true,
  'djexport.progress': true,
  'djexport.finished': true,
  'discogs.progress': true,
  'discogs.finished': true,
  'playlists.changed': true,
  'usb.progress': true,
  'usb.finished': true,
  'sources.changed': true,
  'sources.imported': true,
  'slskd.setup': true,
  'preview.changed': true,
  'playlist.changed': true,
  'playlist.item': true,
  'playlist.finished': true,
} satisfies Record<CoreEventName, true>) as CoreEventName[];

interface CallResponse {
  ok: boolean;
  result?: unknown;
  error?: { code: string; message: string };
}

export function createHttpBridge(base = ''): Bridge {
  const call = async (method: string, params?: unknown): Promise<unknown> => {
    let res: Response;
    try {
      res = await fetch(`${base}/api/call`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Sonora-Client': 'web' },
        body: JSON.stringify({ method, params: params ?? {} }),
      });
    } catch {
      throw new CoreError('bridge_unavailable', 'The SONORA dev bridge is not running. Start it with `npm run dev:web`.');
    }
    if (!res.ok) {
      throw new CoreError('bridge_error', `The dev bridge answered ${res.status} ${res.statusText}.`);
    }
    const body = (await res.json()) as CallResponse;
    if (!body.ok) {
      throw new CoreError(body.error?.code ?? 'unknown_error', body.error?.message ?? 'Unknown error');
    }
    return body.result;
  };

  const subscribe = (handler: (name: string, payload: unknown) => void): Promise<() => void> => {
    const source = new EventSource(`${base}/api/events`);
    // EventSource reconnects by itself after a dropped connection (the bridge
    // restarted, the computer slept); what was sent meanwhile is lost (B96).
    let opened = false;
    const onOpen = () => {
      if (opened) handler(EVENTS_RECONNECTED, null);
      opened = true;
    };
    source.addEventListener('open', onOpen);
    const listeners = EVENT_NAMES.map((name) => {
      const listener = (e: MessageEvent<string>) => {
        try {
          handler(name, JSON.parse(e.data) as unknown);
        } catch (err) {
          console.error('bad event payload', name, err);
        }
      };
      source.addEventListener(name, listener);
      return [name, listener] as const;
    });
    return Promise.resolve(() => {
      source.removeEventListener('open', onOpen);
      for (const [name, listener] of listeners) source.removeEventListener(name, listener);
      source.close();
    });
  };

  const onFileDrop = (handlers: DropHandlers): Promise<() => void> => {
    // Stop the browser from navigating to dropped files, and tell the
    // interface a drop happened (without paths, which browsers do not expose).
    let depth = 0;
    const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes('Files');
    const enter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth += 1;
      handlers.onHover(true);
    };
    const over = (e: DragEvent) => {
      if (hasFiles(e)) e.preventDefault();
    };
    const leave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) handlers.onHover(false);
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = 0;
      handlers.onHover(false);
      handlers.onDrop([]);
    };
    window.addEventListener('dragenter', enter);
    window.addEventListener('dragover', over);
    window.addEventListener('dragleave', leave);
    window.addEventListener('drop', drop);
    return Promise.resolve(() => {
      window.removeEventListener('dragenter', enter);
      window.removeEventListener('dragover', over);
      window.removeEventListener('dragleave', leave);
      window.removeEventListener('drop', drop);
    });
  };

  return {
    kind: 'http',
    platform: 'web',
    native: false,
    updater: simulatedUpdater(call),
    call,
    subscribe,
    pickFiles: () => Promise.resolve(null),
    pickFolders: () => Promise.resolve(null),
    pickSavePath: () => Promise.resolve(null),
    pickFolder: () => Promise.resolve(null),
    openExternal: (url: string) => {
      window.open(url, '_blank', 'noopener,noreferrer');
      return Promise.resolve();
    },
    mediaUrl: (prepared: PreparedPlayback) => `${base}/api/audio?path=${encodeURIComponent(prepared.path)}`,
    previewUrl: (id: number) => `${base}/api/preview?id=${id}`,
    reveal: async (path: string) => {
      await call('system.reveal', { path });
    },
    onFileDrop,
  };
}

/**
 * The browser cannot update anything. For developing and testing the update
 * screens, `?simulate-update=<version>` in the address makes checks find that
 * version; installing it walks through the steps and reloads the page.
 */
function simulatedUpdater(call: (method: string, params?: unknown) => Promise<unknown>): Updater | null {
  const version = typeof location === 'undefined' ? null : new URLSearchParams(location.search).get('simulate-update');
  if (!version) return null;
  const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
  return {
    async check() {
      await wait(400);
      const info = (await call('app.info')) as { version: string };
      return {
        version,
        currentVersion: info.version,
        date: new Date().toISOString(),
        notes: '### New\n- Playlists you can sort by dragging\n- Send playlists to a USB stick\n\n### Fixed\n- Pausing while a track loads',
      };
    },
    async install(onStep: (s: UpdateStep) => void) {
      const total = 24_000_000;
      for (let downloaded = 0; downloaded <= total; downloaded += 3_000_000) {
        onStep({ step: 'downloading', downloaded, total });
        await wait(150);
      }
      onStep({ step: 'installing' });
      await wait(600);
      onStep({ step: 'restarting' });
      await wait(400);
      location.reload();
    },
    restart() {
      location.reload();
      return Promise.resolve();
    },
  };
}
