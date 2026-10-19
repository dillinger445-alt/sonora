/**
 * Desktop implementation of {@link Bridge}: Tauri IPC to the core, native
 * open/save dialogs, file drag-and-drop with real paths and Reveal in
 * Finder / File Explorer.
 *
 * The Rust side (`src-tauri/src/lib.rs`) exposes a single command,
 * `sonora_call`, and forwards every core event on the `sonora://event`
 * channel as `{ name, payload }`.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open, save } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { CoreError } from './bridge.ts';
import type { Bridge, DropHandlers, Platform } from './bridge.ts';

const EVENT_CHANNEL = 'sonora://event';

/** Extensions offered by the "Add files" dialog. The core identifies formats by content, not by name. */
export const AUDIO_EXTENSIONS: readonly string[] = [
  'wav', 'wave', 'bwf', 'flac', 'aif', 'aiff', 'aifc', 'mp3', 'm4a', 'mp4', 'aac', 'alac',
  'ogg', 'oga', 'opus', 'wma', 'wv', 'ape', 'caf', 'mka', 'webm',
];

interface CoreEventMessage {
  name: string;
  payload: unknown;
}

function detectPlatform(): Platform {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  if (/Mac|iPhone|iPad/i.test(ua)) return 'macos';
  if (/Windows/i.test(ua)) return 'windows';
  return 'linux';
}

/** Turns anything an IPC call rejects with into a `CoreError`. */
export function toCoreError(error: unknown, fallbackCode = 'ipc_error'): CoreError {
  if (error instanceof CoreError) return error;
  if (typeof error === 'object' && error !== null) {
    const { code, message } = error as { code?: unknown; message?: unknown };
    if (typeof code === 'string' && typeof message === 'string') return new CoreError(code, message);
    if (typeof message === 'string') return new CoreError(fallbackCode, message);
  }
  if (typeof error === 'string' && error.length > 0) return new CoreError(fallbackCode, error);
  return new CoreError(fallbackCode, 'The desktop shell did not respond as expected.');
}

/** Runs a native-integration call and reports failures as `CoreError`s. */
async function native<T>(code: string, action: () => Promise<T>): Promise<T> {
  try {
    return await action();
  } catch (error) {
    throw toCoreError(error, code);
  }
}

export function createTauriBridge(): Bridge {
  const platform = detectPlatform();
  // GTK file filters are case-sensitive; macOS and Windows ignore case.
  const extensions =
    platform === 'linux' ? [...AUDIO_EXTENSIONS, ...AUDIO_EXTENSIONS.map((e) => e.toUpperCase())] : [...AUDIO_EXTENSIONS];

  return {
    kind: 'tauri',
    platform,
    native: true,

    async call(method: string, params?: unknown): Promise<unknown> {
      try {
        return await invoke<unknown>('sonora_call', { method, params: params ?? null });
      } catch (error) {
        throw toCoreError(error);
      }
    },

    async subscribe(handler: (name: string, payload: unknown) => void): Promise<() => void> {
      return listen<CoreEventMessage>(EVENT_CHANNEL, (event) => {
        handler(event.payload.name, event.payload.payload);
      });
    },

    pickFiles(): Promise<string[] | null> {
      return native('dialog_failed', () =>
        open({
          title: 'Add audio files',
          multiple: true,
          directory: false,
          filters: [{ name: 'Audio files', extensions }],
        }),
      );
    },

    pickFolders(): Promise<string[] | null> {
      return native('dialog_failed', () =>
        open({
          title: 'Add music folders',
          multiple: true,
          directory: true,
        }),
      );
    },

    pickSavePath(defaultName: string): Promise<string | null> {
      return native('dialog_failed', () =>
        save({
          title: 'Export CSV',
          defaultPath: defaultName,
          filters: [{ name: 'CSV', extensions: ['csv'] }],
        }),
      );
    },

    reveal(path: string): Promise<void> {
      return native('reveal_failed', () => revealItemInDir(path));
    },

    async onFileDrop(handlers: DropHandlers): Promise<() => void> {
      let hovering = false;
      const setHover = (over: boolean) => {
        if (over !== hovering) {
          hovering = over;
          handlers.onHover(over);
        }
      };
      return getCurrentWebview().onDragDropEvent((event) => {
        const drag = event.payload;
        switch (drag.type) {
          case 'enter':
            // Dragged text or links carry no paths; only react to files.
            setHover(drag.paths.length > 0);
            break;
          case 'over':
            break;
          case 'leave':
            setHover(false);
            break;
          case 'drop':
            setHover(false);
            if (drag.paths.length > 0) handlers.onDrop(drag.paths);
            break;
        }
      });
    },
  };
}
