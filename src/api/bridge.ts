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
  /** Reveals a file in Finder / File Explorer. */
  reveal(path: string): Promise<void>;
  /** Registers file drag-and-drop handlers; resolves to an unregister function. */
  onFileDrop(handlers: DropHandlers): Promise<() => void>;
}
