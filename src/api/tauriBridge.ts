/**
 * Desktop implementation of {@link Bridge}: Tauri IPC to the core, native
 * open/save dialogs, file drag-and-drop with real paths and Reveal in
 * Finder / File Explorer.
 *
 * The Rust side (`src-tauri/src/lib.rs`) exposes a single command,
 * `sonora_call`, and forwards every core event on the `sonora://event`
 * channel as `{ name, payload }`.
 */

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open, save } from '@tauri-apps/plugin-dialog';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { CoreError } from './bridge.ts';
import type { Bridge, DropHandlers, Platform, UpdateInfo, Updater, UpdateStep } from './bridge.ts';
import type { PreparedPlayback } from './types.ts';

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

/**
 * Updates from GitHub releases through Tauri's updater: the release's
 * `latest.json` names a signed installer per platform, and the signature is
 * checked against the public key in tauri.conf.json before anything runs.
 */
function createUpdater(): Updater {
  let found: Update | null = null;
  return {
    async check(): Promise<UpdateInfo | null> {
      const update = await native('update_check_failed', () => check({ timeout: 30_000 }));
      const previous = found;
      found = update;
      if (previous && previous !== update) void previous.close().catch(() => undefined);
      return update ? { version: update.version, currentVersion: update.currentVersion, date: update.date ?? null, notes: update.body ?? null } : null;
    },

    async install(onStep: (s: UpdateStep) => void): Promise<void> {
      const update = found;
      if (!update) throw new CoreError('update_missing', 'Check for updates again: the release found earlier is no longer at hand.');
      let downloaded = 0;
      let total: number | null = null;
      onStep({ step: 'downloading', downloaded, total });
      await native('update_download_failed', () =>
        update.download(
          (e) => {
            if (e.event === 'Started') total = e.data.contentLength ?? null;
            else if (e.event === 'Progress') downloaded += e.data.chunkLength;
            onStep({ step: 'downloading', downloaded, total });
          },
          { timeout: 15 * 60_000 },
        ),
      );
      onStep({ step: 'installing' });
      // Scans, exports and lookups stop before the files are replaced.
      await native('update_install_failed', () => invoke('prepare_update'));
      // Windows: the installer replaces this version, ends this process and
      // starts the new one. macOS: the app bundle is replaced in place.
      await native('update_install_failed', () => update.install());
      onStep({ step: 'restarting' });
      await native('update_restart_failed', () => relaunch());
    },

    restart(): Promise<void> {
      return native('update_restart_failed', () => relaunch());
    },
  };
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
    updater: platform === 'linux' ? null : createUpdater(),

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

    pickFolder(title: string): Promise<string | null> {
      return native('dialog_failed', () => open({ title, multiple: false, directory: true }));
    },

    reveal(path: string): Promise<void> {
      return native('reveal_failed', () => revealItemInDir(path));
    },

    openExternal(url: string): Promise<void> {
      return native('open_failed', () => openUrl(url));
    },

    mediaUrl(prepared: PreparedPlayback): string {
      // The shell serves only files the core prepared for playback, by id.
      if (!prepared.stream) throw new CoreError('playback_failed', 'This file could not be opened for playback.');
      return convertFileSrc(prepared.stream, 'sonora');
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
