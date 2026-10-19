/**
 * Typed access to the core API. Every method maps 1:1 to a core method
 * (crates/sonora-core/src/api.rs); validation happens in the core.
 */

import type { Bridge } from './bridge.ts';
import type {
  AppInfo,
  CacheStats,
  CoreEventName,
  CoreEvents,
  DuplicateList,
  Facets,
  Folder,
  LibraryFilter,
  Overview,
  QueryResult,
  ScanItemFilter,
  ScanItemRow,
  ScanRecord,
  ScanStatus,
  Settings,
  Sort,
  TrackDetail,
} from './types.ts';

export type EventHandler = <K extends CoreEventName>(name: K, payload: CoreEvents[K]) => void;

/** What a request to analyze tracks did. */
export interface QueueResult {
  /** Tracks put in the queue (including ones the running scan already finished). */
  queued: number;
  /** Tracks that were already waiting or being analyzed. */
  alreadyQueued: number;
  /** Requested tracks whose files are missing (`tracks.rescan` only). */
  missing?: number;
}

export class Client {
  readonly bridge: Bridge;

  constructor(bridge: Bridge) {
    this.bridge = bridge;
  }

  private call<T>(method: string, params?: object): Promise<T> {
    return this.bridge.call(method, params ?? {}) as Promise<T>;
  }

  subscribe(handler: (name: string, payload: unknown) => void): Promise<() => void> {
    return this.bridge.subscribe(handler);
  }

  // App
  info = () => this.call<AppInfo>('app.info');
  overview = () => this.call<Overview>('app.overview');

  // Library
  importPaths = (paths: string[], force = false) => this.call<{ accepted: boolean; paths: number }>('library.import', { paths, force });
  query = (filter: LibraryFilter, sort: Sort, offset: number, limit: number) =>
    this.call<QueryResult>('library.query', { filter, sort, offset, limit });
  ids = (filter: LibraryFilter) => this.call<{ ids: number[] }>('library.ids', { filter });
  facets = () => this.call<Facets>('library.facets');

  // Tracks
  /** `visuals: false` leaves out the visualization data (for quick previews). */
  track = (id: number, opts?: { visuals?: boolean }) => this.call<TrackDetail>('tracks.get', { id, ...opts });
  rescan = (ids: number[]) => this.call<QueueResult>('tracks.rescan', { ids });
  retryFailed = (ids?: number[]) => this.call<QueueResult>('tracks.retryFailed', ids ? { ids } : {});
  analyzePending = () => this.call<QueueResult>('tracks.analyzePending');
  remove = (ids: number[]) => this.call<{ removed: number }>('tracks.remove', { ids });
  revealTrack = (id: number) => this.call<{ ok: boolean }>('tracks.reveal', { id });

  // Folders
  folders = () => this.call<Folder[]>('folders.list');
  rescanFolders = (ids?: number[]) => this.call<{ accepted: boolean }>('folders.rescan', ids ? { ids } : {});
  removeFolder = (id: number, removeTracks: boolean) => this.call<{ removedTracks: number }>('folders.remove', { id, removeTracks });

  // Scans
  scanStatus = () => this.call<ScanStatus>('scan.status');
  pause = () => this.call<{ ok: boolean }>('scan.pause');
  resume = () => this.call<{ ok: boolean }>('scan.resume');
  cancel = () => this.call<{ ok: boolean }>('scan.cancel');
  history = (limit = 100) => this.call<ScanRecord[]>('scan.history', { limit });
  scanItems = (id: number, filter: ScanItemFilter, offset = 0, limit = 200) =>
    this.call<{ total: number; rows: ScanItemRow[] }>('scan.items', { id, filter, offset, limit });

  // Duplicates
  duplicates = () => this.call<DuplicateList>('duplicates.list');
  scanDuplicates = () => this.call<{ accepted: boolean }>('duplicates.scan');

  // Settings and maintenance
  settings = () => this.call<Settings>('settings.get');
  updateSettings = (values: Partial<Settings>) => this.call<Settings>('settings.update', { values });
  refreshTools = () => this.call<AppInfo>('tools.refresh');
  exportCsv = (target: { ids: number[] } | { filter: LibraryFilter }, path?: string) =>
    this.call<{ path?: string; csv?: string; rows: number }>('export.csv', path ? { ...target, path } : target);
  cacheStats = () => this.call<CacheStats>('cache.stats');
  clearVisuals = () => this.call<{ cleared: number }>('cache.clearVisuals');
  backup = (path: string) => this.call<{ path: string }>('db.backup', { path });
}
