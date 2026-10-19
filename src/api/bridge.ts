/**
 * The bridge between the interface and the SONORA core.
 *
 * Two implementations exist:
 * - `tauriBridge.ts` — the desktop app (Tauri IPC, native dialogs,
 *   drag-and-drop with real file paths, Reveal in Finder/Explorer);
 * - `httpBridge.ts` — browser development and end-to-end tests against the
 *   local `sonora-devserver` (same core, same API, over HTTP + SSE).
 *
 * Everything else in the UI talks only to this interface.
 */

import type { PreparedPlayback } from './types.ts';

export type Platform = 'macos' | 'windows' | 'linux' | 'web';

/** Error raised by the core: a stable machine code plus a readable message. */
export class CoreError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
    this.name = 'CoreError';
  }
}

export interface DropHandlers {
  /** Paths dropped onto the window. */
  onDrop: (paths: string[]) => void;
  /** Whether files are currently dragged over the window. */
  onHover: (over: boolean) => void;
}

/** A newer SONORA release found on GitHub. */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  /** Release date (ISO 8601), when the release gives one. */
  date: string | null;
  /** Release notes as written on GitHub (Markdown). */
  notes: string | null;
}

/** Where an installation stands. */
export type UpdateStep = { step: 'downloading'; downloaded: number; total: number | null } | { step: 'installing' } | { step: 'restarting' };

/** In-app updates from SONORA's GitHub releases. */
export interface Updater {
  /** Asks GitHub for the newest release; `null` when this version is the newest. */
  check(): Promise<UpdateInfo | null>;
  /**
   * Downloads the release `check` found, verifies its signature, stops the
   * core's background work, installs it over this version and restarts
   * SONORA. Settings and the library stay where they are. Resolves only when
   * the restart did not happen.
   */
  install(onStep: (s: UpdateStep) => void): Promise<void>;
  /** Restarts SONORA (after an installation that stopped half-way). */
  restart(): Promise<void>;
}

export interface Bridge {
  readonly kind: 'tauri' | 'http';
  readonly platform: Platform;
  /** True when native dialogs and drag-and-drop with file paths are available. */
  readonly native: boolean;
  /** Calls a core API method; rejects with `CoreError`. */
  call(method: string, params?: unknown): Promise<unknown>;
  /** Subscribes to core events; resolves to an unsubscribe function. */
  subscribe(handler: (name: string, payload: unknown) => void): Promise<() => void>;
  /** Native multi-file picker filtered to audio files; `null` if cancelled or unavailable. */
  pickFiles(): Promise<string[] | null>;
  /** Native folder picker (multiple allowed); `null` if cancelled or unavailable. */
  pickFolders(): Promise<string[] | null>;
  /** Native save dialog for a CSV file; `null` if cancelled or unavailable. */
  pickSavePath(defaultName: string): Promise<string | null>;
  /** Native picker for one folder (export destination); `null` if cancelled or unavailable. */
  pickFolder(title: string): Promise<string | null>;
  /** Opens a web page in the default browser (Discogs pages only). */
  openExternal(url: string): Promise<void>;
  /** URL the audio element can play for a file the core prepared for playback. */
  mediaUrl(prepared: PreparedPlayback): string;
  /** Reveals a file in Finder / File Explorer. */
  reveal(path: string): Promise<void>;
  /** Registers file drag-and-drop handlers; resolves to an unregister function. */
  onFileDrop(handlers: DropHandlers): Promise<() => void>;
  /** In-app updates; `null` where SONORA cannot update itself (the browser bridge). */
  readonly updater: Updater | null;
}
