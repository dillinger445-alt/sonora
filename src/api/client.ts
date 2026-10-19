/**
 * Typed access to the core API. Every method maps 1:1 to a core method
 * (crates/sonora-core/src/api.rs); validation happens in the core.
 */

import type { Bridge } from './bridge.ts';
import type {
  AppInfo,
  ArrangeResult,
  CacheStats,
  CoreEventName,
  CoreEvents,
  DiscogsStatus,
  Drive,
  DuplicateList,
  ExportOptions,
  ExportPlan,
  ExportReport,
  Facets,
  Folder,
  KeyNotation,
  LibraryFilter,
  OrganizeBy,
  OrganizeResult,
  Overview,
  PlaylistKind,
  PlaylistNode,
  PreparedPlayback,
  QueryResult,
  ScanItemFilter,
  ScanItemRow,
  ScanRecord,
  ScanStatus,
  Settings,
  SmartRules,
  Sort,
  TrackDetail,
  TrackRow,
  UsbOptions,
  UsbPlan,
  UsbReport,
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

/** A change to tracks' tempo, key or genre (`null` removes an edit). */
export interface DjEdit {
  bpm?: number | null;
  bpmFactor?: 0.5 | 2;
  key?: number | null;
  genre?: string | null;
  reset?: boolean;
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
  editDj = (ids: number[], edit: DjEdit) => this.call<{ updated: number }>('tracks.editDj', { ids, ...edit });
  analyzeDj = () => this.call<QueueResult>('tracks.analyzeDj');
  preparePlayback = (id: number, playable: string[], decode = false) => this.call<PreparedPlayback>('playback.prepare', { id, playable, decode });

  // DJ export
  exportPlan = (ids: number[], options: ExportOptions) => this.call<ExportPlan>('djexport.plan', { ids, ...options });
  exportStart = (ids: number[], options: ExportOptions, folder: string, playlistName: string) =>
    this.call<{ accepted: boolean; total: number }>('djexport.start', { ids, ...options, folder, playlistName });
  exportCancel = () => this.call<{ ok: boolean }>('djexport.cancel');
  exportStatus = () => this.call<{ running: boolean; last: ExportReport | null }>('djexport.status');

  // Playlists
  playlists = () => this.call<{ nodes: PlaylistNode[] }>('playlists.list');
  createPlaylist = (kind: PlaylistKind, name: string, opts: { parentId?: number | null; index?: number; rules?: SmartRules; trackIds?: number[] } = {}) =>
    this.call<PlaylistNode>('playlists.create', { kind, name, ...opts });
  updatePlaylist = (id: number, values: { name?: string; rules?: SmartRules }) => this.call<PlaylistNode>('playlists.update', { id, ...values });
  movePlaylist = (id: number, parentId: number | null, index: number) => this.call<{ ok: boolean }>('playlists.move', { id, parentId, index });
  deletePlaylist = (id: number) => this.call<{ deleted: number }>('playlists.delete', { id });
  duplicatePlaylist = (id: number) => this.call<PlaylistNode>('playlists.duplicate', { id });
  playlistTracks = (id: number) => this.call<{ id: number; rows: TrackRow[]; total: number }>('playlists.tracks', { id });
  addToPlaylist = (id: number, trackIds: number[], index?: number) =>
    this.call<{ added: number; already: number }>('playlists.addTracks', index == null ? { id, trackIds } : { id, trackIds, index });
  removeFromPlaylist = (id: number, trackIds: number[]) => this.call<{ removed: number }>('playlists.removeTracks', { id, trackIds });
  reorderPlaylist = (id: number, trackIds: number[]) => this.call<{ ok: boolean }>('playlists.reorder', { id, trackIds });
  moveInPlaylist = (id: number, trackIds: number[], index: number) => this.call<{ ok: boolean }>('playlists.moveTracks', { id, trackIds, index });
  arrangePlaylist = (id: number, opts: { start?: number | null; tempo?: 'any' | 'rising' } = {}) => this.call<ArrangeResult>('playlists.arrange', { id, ...opts });
  freezePlaylist = (id: number) => this.call<{ tracks: number }>('playlists.freeze', { id });
  organize = (by: OrganizeBy, opts: { smart?: boolean; trackIds?: number[]; keyNotation?: KeyNotation } = {}) =>
    this.call<OrganizeResult>('playlists.organize', { by, ...opts });
  playlistsContaining = (trackId: number) => this.call<{ ids: number[] }>('playlists.containing', { trackId });

  // USB
  drives = () => this.call<{ drives: Drive[] }>('usb.drives');
  usbPlan = (root: string, playlistIds: number[], options: UsbOptions) => this.call<UsbPlan>('usb.plan', { root, playlistIds, ...options });
  usbStart = (root: string, playlistIds: number[], options: UsbOptions) => this.call<{ accepted: boolean; total: number }>('usb.start', { root, playlistIds, ...options });
  usbCancel = () => this.call<{ ok: boolean }>('usb.cancel');
  usbStatus = () => this.call<{ running: boolean; last: UsbReport | null }>('usb.status');

  // Discogs
  discogsStatus = () => this.call<DiscogsStatus>('discogs.status');
  discogsSetToken = (token: string | null) => this.call<{ configured: boolean }>('discogs.setToken', { token });
  discogsTest = () => this.call<{ username: string }>('discogs.test');
  discogsLookup = (ids: number[], overwrite: boolean) => this.call<{ accepted: boolean; total: number }>('discogs.lookup', { ids, overwrite });
  discogsCancel = () => this.call<{ ok: boolean }>('discogs.cancel');

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
