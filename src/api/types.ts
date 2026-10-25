/**
 * Shapes returned by the SONORA core API (see crates/sonora-core/src/api.rs).
 * Keep in sync with the Rust serializers; numbers are already rounded there.
 */

export type ScoreKind = 'overall' | 'dj' | 'listening';
export type TrackStatus = 'pending' | 'analyzing' | 'done' | 'failed';
export type Evidence = 'fact' | 'strong' | 'weak' | 'inconclusive';
export type Severity = 'info' | 'low' | 'moderate' | 'high' | 'critical';
export type CategoryId = 'source' | 'artifacts' | 'clipping' | 'noise' | 'frequency' | 'general';
export type FlagId =
  | 'attention'
  | 'review'
  | 'clipping'
  | 'lossy'
  | 'low_bitrate'
  | 'source_concern'
  | 'artifacts'
  | 'integrity'
  | 'mislabeled'
  | 'phase'
  | 'noise'
  | 'upsampled'
  | 'padded'
  | 'silent'
  | 'inconclusive'
  | 'bandwidth';

export interface TrackRow {
  id: number;
  path: string;
  folderId: number | null;
  fileName: string;
  ext: string;
  size: number;
  mtime: number;
  status: TrackStatus;
  errorCode: string | null;
  errorMessage: string | null;
  addedAt: number;
  analyzedAt: number | null;
  missing: boolean;
  missingSince: number | null;
  title: string | null;
  displayTitle: string;
  artist: string | null;
  album: string | null;
  genre: string | null;
  bpm: number | null;
  key: string | null;
  container: string | null;
  codec: string | null;
  codecName: string | null;
  format: string | null;
  lossless: boolean | null;
  sampleRate: number | null;
  bitDepth: number | null;
  bitrateKbps: number | null;
  channels: number | null;
  duration: number | null;
  integratedLufs: number | null;
  truePeak: number | null;
  cutoffHz: number | null;
  upperHz: number | null;
  overall: number | null;
  dj: number | null;
  listening: number | null;
  confidence: number | null;
  confidenceLabel: string | null;
  verdict: string | null;
  flags: string[];
  modelVersion: string | null;
  /** Tempo, key (0–11 major, 12–23 minor; tonic 0 = C) and genre in use. */
  djBpm: number | null;
  djBpmSource: DjSource | null;
  djKey: number | null;
  djKeySource: DjSource | null;
  djGenre: string | null;
  djGenreSource: DjSource | null;
  bpmDetected: number | null;
  bpmConfidence: number | null;
  tempoVariable: boolean | null;
  keyDetected: number | null;
  keyConfidence: number | null;
  keySecond: number | null;
  /** Release page of a genre found on Discogs. */
  discogsUrl: string | null;
  /** The stored analysis includes tempo and key detection. */
  djAnalyzed: boolean;
}

/** Where a tempo, key or genre comes from. */
export type DjSource = 'edit' | 'detected' | 'tag' | 'folder' | 'discogs';

export type KeyNotation = 'camelot' | 'openkey' | 'musical' | 'sharps' | 'flats';

export interface AppInfo {
  version: string;
  engineVersion: string;
  modelVersion: string;
  measurementSchema: number;
  dataDir: string;
  dbPath: string;
  sqlite: string;
  ffmpeg: { available: boolean; path: string | null; version: string | null };
  platform: string;
  arch: string;
  cpus: number;
  nativeFormats: string[];
  ffmpegFormats: string[];
}

export interface ScanSummary {
  avgOverall: number | null;
  avgDj: number | null;
  avgListening: number | null;
  clipping: number;
  sourceConcerns: number;
  integrity: number;
  attention: number;
  audioSeconds: number | null;
  elapsedMs: number;
  /** Set when the scan had nothing to analyze. */
  message?: string;
}

/** A row of the `scans` table (history). */
export interface ScanRecord {
  id: number;
  kind: string;
  state: string;
  startedAt: number;
  finishedAt: number | null;
  total: number;
  completed: number;
  failed: number;
  skipped: number;
  roots: string[];
  cancelled?: number;
  summary?: ScanSummary | null;
}

export type ScanPhase = 'idle' | 'indexing' | 'running' | 'paused' | 'cancelling';

export interface ScanWorker {
  trackId: number;
  fileName: string;
  path: string;
  progress: number;
  elapsedMs: number;
}

/** Live state of the scan manager (`scan.status` and scan events). */
export interface ScanStatus {
  id: number | null;
  /**
   * A phase while live; `completed`, `cancelled` or `interrupted` (the app
   * quit mid-scan) in `scan.finished`.
   */
  state: ScanPhase | 'completed' | 'cancelled' | 'interrupted';
  kind: string;
  startedAt: number | null;
  total: number;
  completed: number;
  failed: number;
  skipped: number;
  cancelled: number;
  /** Items finished in any way (analyzed, failed, cancelled or skipped). */
  done: number;
  pending: number;
  running: number;
  current: ScanWorker[];
  elapsedMs: number;
  etaMs: number | null;
  ratePerMin: number | null;
  workers: number;
  /** What the indexer is doing, while it runs. */
  indexing: string | null;
  summary?: ScanSummary;
}

export interface ScanItemEvent {
  scanId: number | null;
  trackId: number;
  outcome: 'done' | 'failed' | 'cancelled' | 'interrupted' | 'skipped';
  error: { code: string; message: string } | null;
  track: TrackRow | null;
}

export interface ImportReport {
  queued: number;
  added: number;
  changed: number;
  unchanged: number;
  relinked: number;
  restored: number;
  markedMissing: number;
  folders: number[];
  errors: string[];
  cancelled: boolean;
  /** Files left out because their tracks were removed from the library. */
  skippedRemoved?: number;
  /** Files chosen by name that are not audio (a cover image, a text file), left out. */
  notAudio?: number;
  /** The files to analyze were not queued (a cancelled scan was still stopping): they wait. */
  waiting?: boolean;
  /** Set when the files were downloads from a source (the person's own Soulseek client). */
  source?: 'soulseek';
}

export interface Overview {
  tracks: {
    total: number;
    analyzed: number;
    pending: number;
    analyzing: number;
    failed: number;
    missing: number;
    /** Waiting tracks whose files are on disk: what "Analyze waiting" starts. */
    pendingPresent: number;
    /** Failed tracks whose files are on disk: what "Retry failed" retries. */
    failedPresent: number;
  };
  folders: number;
  averages: Record<ScoreKind, number | null>;
  totalDuration: number | null;
  attention: number;
  clipping: number;
  sourceConcerns: number;
  integrity: number;
  lossy: number;
  distribution: { kind: ScoreKind; bins: number[] }[];
  formats: { format: string; count: number; avgOverall: number | null }[];
  recentTracks: TrackRow[];
  recentScans: ScanRecord[];
}

export interface Facets {
  formats: { id: string; count: number }[];
  statuses: { id: string; count: number }[];
  missing: number;
  flags: { id: string; count: number }[];
  lossless: number;
  lossy: number;
  genres: { id: string; count: number }[];
  keys: { key: number; count: number }[];
  /** Analyzed tracks whose analysis predates tempo and key detection. */
  djPending: number;
}

export interface LibraryFilter {
  search?: string;
  scoreKind?: ScoreKind;
  minScore?: number;
  maxScore?: number;
  /** Display formats ("FLAC", "MP3"…) or the groups "lossless" / "lossy". */
  formats?: string[];
  /** Tracks having any of these flags. */
  flags?: string[];
  /** "done" | "pending" | "analyzing" | "failed" | "missing". */
  statuses?: string[];
  folderId?: number;
  attention?: boolean;
  bpmMin?: number;
  bpmMax?: number;
  /** Also match half and double the BPM range. */
  bpmHalfDouble?: boolean;
  /** Keys 0–23. */
  keys?: number[];
  genres?: string[];
}

export type SortKey =
  | 'title'
  | 'artist'
  | 'album'
  | 'duration'
  | 'format'
  | 'sampleRate'
  | 'bitDepth'
  | 'bitrate'
  | 'overall'
  | 'dj'
  | 'listening'
  | 'confidence'
  | 'lufs'
  | 'truePeak'
  | 'cutoff'
  | 'size'
  | 'added'
  | 'analyzed'
  | 'path'
  | 'status'
  | 'bpm'
  | 'key'
  | 'genre';

export interface Sort {
  key: SortKey;
  desc: boolean;
}

export interface QueryResult {
  total: number;
  offset: number;
  rows: TrackRow[];
}

export interface Folder {
  id: number;
  path: string;
  name: string;
  addedAt: number;
  lastScanAt: number | null;
  available: boolean;
  tracks: number;
  analyzed: number;
  missing: number;
  failed: number;
  /** Tracks that failed or whose file is missing, each counted once. */
  problems: number;
  avgOverall: number | null;
  bytes: number;
  /** Files in it removed from the library, which rescans leave out. */
  removed: number;
}

export interface Settings {
  workers: number;
  defaultScore: ScoreKind;
  rescanOnLaunch: 'off' | 'changed';
  reanalyzeChanged: boolean;
  skipHidden: boolean;
  followSymlinks: boolean;
  maxDurationMin: number;
  keepVisuals: boolean;
  theme: 'dark';
  keyNotation: KeyNotation;
  /** Tempos are shown within this range, e.g. [70, 180]. */
  bpmRange: [number, number];
  genreFromFolders: 'off' | 'top' | 'parent';
  /** Desktop app: look for a newer release on GitHub on launch and every few hours. */
  checkUpdates: boolean;
  /** The person's own Soulseek client's download folder, watched (set through `sources.configure`). */
  soulseekFolder: string | null;
  /** Hold each download for the person to add or keep out. */
  soulseekReview: boolean;
  /** slskd's web address. */
  slskdUrl: string;
  /** slskd set up by SONORA runs in the background from login on, or only while SONORA is open. */
  slskdBackground: boolean;
  /** SONORA's slskd was stopped by the person and stays stopped until started. */
  slskdStopped: boolean;
  /** How far a Soulseek preview reads ahead of what plays (or the player has buffered), in seconds. */
  previewAhead: number;
  /** Most disk space previews use at a time, in megabytes. */
  previewCacheMb: number;
  /** How a YouTube playlist's tracks are picked from Soulseek results. */
  playlistRules: PlaylistRules;
}

// ------------------------------------------------- YouTube playlists

/** Lossless formats a playlist download may accept, by extension. */
export type LosslessFormat = 'flac' | 'aiff' | 'wav' | 'alac' | 'wv' | 'ape';
export type PlaylistQuality = 'cd' | 'highest' | 'any';
export type PlaylistMode = 'automatic' | 'review';

/** The rules a YouTube playlist download picks files by (Settings → Playlist download rules). */
export interface PlaylistRules {
  /** Accepted lossless formats, best first. */
  formats: LosslessFormat[];
  /** Lossy files too, after every lossless format, from `minBitrate` kbps up. */
  lossy: boolean;
  minBitrate: number;
  /** Prefer 16-bit/44.1 kHz, prefer the highest resolution, or any. */
  quality: PlaylistQuality;
  maxBitDepth: number | null;
  maxSampleRate: number | null;
  /** Reject files far smaller than a lossless file of that length should be. */
  sizeCheck: boolean;
  maxSizeMb: number | null;
  /** Only people with a free upload slot. */
  freeSlot: boolean;
  /** Otherwise, a queue of at most this many downloads. */
  maxQueue: number;
  /** Reported upload speed, in kB/s (0: any). */
  minSpeedKbps: number;
  /** Share of the title's words that must be in the file's name (0–1). */
  minOverlap: number;
  requireArtist: boolean;
  /** Reject names that say remix, edit, live, radio or instrumental when the title does not. */
  rejectVariants: boolean;
  mode: PlaylistMode;
  maxDownloads: number;
  retryNotFound: boolean;
  /** How long each search may run, 15–25 s. */
  searchSeconds: number;
}

export type PlaylistRunState = 'fetching' | 'draft' | 'running' | 'paused' | 'review' | 'waiting' | 'finished' | 'cancelled' | 'failed';
export type PlaylistItemStatus = 'pending' | 'searching' | 'found' | 'not_found' | 'downloading' | 'verifying' | 'done' | 'failed';

export interface PlaylistCounts {
  total: number;
  included: number;
  leftOut: number;
  pending: number;
  searching: number;
  found: number;
  notFound: number;
  downloading: number;
  verifying: number;
  done: number;
  failed: number;
}

/** One playlist being downloaded (`playlist.get`, `playlist.list`). */
export interface PlaylistRun {
  id: number;
  createdAt: number;
  updatedAt: number;
  state: PlaylistRunState;
  /** The link or id as pasted. */
  source: string;
  playlistId: string | null;
  title: string | null;
  /** The rules the run was started with. */
  rules: PlaylistRules | null;
  /** Review mode: the picks were confirmed. */
  confirmed: boolean;
  passes: number;
  /** When tracks not found are looked for again. */
  retryAt: number | null;
  error: string | null;
  finishedAt: number | null;
  counts: PlaylistCounts;
  /** The track being searched for. */
  current: { itemId: number; title: string; variation: number | null } | null;
  /** Only from `playlist.get`. */
  items?: PlaylistItem[];
}

/** A file from the results, judged by the rules. */
export interface PlaylistCandidate {
  username: string;
  filename: string;
  name: string;
  ext: string;
  size: number;
  bitDepth: number | null;
  sampleRate: number | null;
  bitRate: number | null;
  length: number | null;
  lossless: boolean;
  freeSlot: boolean;
  queue: number;
  speed: number;
  /** Share of the title's words in the file's name. */
  overlap: number;
  /** Why it is out, or null when it passed. */
  rejected: string | null;
}

/** One video of the playlist and what became of it. */
export interface PlaylistItem {
  id: number;
  runId: number;
  seq: number;
  videoId: string;
  url: string;
  ytTitle: string;
  channel: string | null;
  seconds: number | null;
  artist: string | null;
  title: string;
  mix: string | null;
  catalogue: string | null;
  included: boolean;
  /** Why it was left out at first (it can be put back). */
  leftOut: string | null;
  status: PlaylistItemStatus;
  /** The search (1–10, see `searchWords`) that found it, or the last one tried. */
  variation: number | null;
  responses: number | null;
  /** Ranked: the ones that passed first. */
  candidates: PlaylistCandidate[];
  /** Index of the pick in `candidates`. */
  chosen: number | null;
  attempts: number;
  transfer: { id: string; username: string | null; filename: string | null; size: number | null; bytes: number } | null;
  /** Fallbacks and other remarks. */
  note: string | null;
  error: string | null;
  trackId: number | null;
  /** The library track once it arrived. */
  track: TrackRow | null;
  updatedAt: number;
}

export interface CacheStats {
  dbBytes: number;
  visualsBytes: number;
  analyses: number;
  dbPath: string;
}

// ------------------------------------------------------------- analysis

export interface Tags {
  title: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  date: string | null;
  bpm: number | null;
  key: string | null;
  comment: string | null;
}

export interface FileFacts {
  fileName: string;
  extension: string;
  fileSize: number;
  container: string;
  containerName: string;
  codecId: string;
  codecName: string;
  codecProfile: string | null;
  lossless: boolean | null;
  floatSamples: boolean;
  sampleRate: number;
  channels: number;
  channelLayout: string | null;
  bitDepth: number | null;
  bitrateKbps: number | null;
  bitrateMode: string | null;
  bitrateModeSource: string | null;
  headerDurationSecs: number | null;
  tags: Tags;
  encoder: string | null;
  encoderLowpassHz: number | null;
  extensionMismatch: string | null;
  metadataSource: string;
}

export interface Edge {
  hz: number;
  dropDb: number;
  edgeDb: number;
  shelfDb: number;
  floorDb: number;
  transitionHz: number;
  floorStdevDb: number;
}

export interface Measurements {
  schema: number;
  engineVersion: string;
  decoder: string;
  sampleKind: string;
  sampleRate: number;
  channels: number;
  analyzedChannels: number;
  duration: number;
  declaredDuration: number | null;
  decodeErrors: number;
  decodeMessages: string[];
  truncated: boolean;
  checksum: string;
  containerBits: number | null;
  effectiveBits: number | null;
  peakDbfs: number;
  channelPeakDbfs: number[];
  truePeakDbtp: number;
  truePeakOversampling: number;
  tpOverFrames: number;
  tpOverEvents: number;
  rmsDbfs: number;
  channelRmsDbfs: number[];
  crestDb: number;
  dcOffset: number[];
  decodedOvers: number;
  clipping: {
    events: number;
    samples: number;
    longestRun: number;
    eventsPerMinute: number;
    ceilingDbfs: number;
    clippedFraction: number;
  } | null;
  integratedLufs: number | null;
  lra: number | null;
  maxMomentaryLufs: number | null;
  maxShortTermLufs: number | null;
  plrDb: number | null;
  stereo: {
    correlation: number;
    lowCorrelation: number | null;
    monoLossDb: number;
    lowMonoLossDb: number | null;
    lowNegativeFraction: number;
    sideToMidDb: number;
    identical: boolean;
    inverted: boolean;
    maxAbsDifference: number;
  } | null;
  silence: {
    leading: number;
    trailing: number;
    silentFraction: number;
    allSilent: boolean;
    allZero: boolean;
    dropoutCount: number;
    dropouts: { start: number; duration: number; preDb: number; postDb: number }[];
    abruptCutoff: { start: number; length: number } | null;
    endingLevelDb: number;
  };
  bandwidth: {
    edge: Edge | null;
    upperHz: number;
    nyquist: number;
    ceilingConsistency: number | null;
    breakthroughP99Db: number | null;
    hfContent: number;
    sideEdge: Edge | null;
    ultrasonicRelDb: number | null;
  };
  artifacts: {
    loHz: number;
    hiHz: number;
    holeRatio: number | null;
    topHoleRatio: number | null;
    sfm: number | null;
    frames: number;
  };
  segments: { start: number; end: number; cutoffHz: number | null; edgeDb: number; upperHz: number; referenceDb: number }[];
  bandwidthChanges: { time: number; fromHz: number; toHz: number }[];
  noise: { quietSeconds: number; quietRmsDb: number | null; quietHfFlatness: number | null };
  bands: { id: string; label: string; loHz: number; hiHz: number; share: number; relDb: number }[];
  /** Missing in analyses made before tempo detection, and when there is no pulse. */
  tempo?: { bpm: number; confidence: number; variable: boolean; candidates: [number, number][] } | null;
  /** Missing in analyses made before key detection, and without tonal material. */
  key?: {
    key: number;
    name: string;
    camelot: string;
    confidence: number;
    second: number;
    tuningCents: number;
    chroma: number[];
    changes: boolean;
  } | null;
  analysisMs: number;
}

export interface Finding {
  id: string;
  category: CategoryId;
  family: string;
  severity: Severity;
  evidence: Evidence;
  confidence: number;
  title: string;
  summary: string;
  detail: string;
  impact: string;
  reliability: string;
  recommendation: string | null;
  penalty: { overall: number; dj: number; listening: number; nominalOverall: number };
  metrics: { label: string; value: string }[];
  times: number[];
}

export interface Subscore {
  id: CategoryId;
  label: string;
  description: string;
  score: number;
  findings: string[];
}

export interface Assessment {
  modelVersion: string;
  scores: Record<ScoreKind, number>;
  subscores: Subscore[];
  deductions: { family: string; overall: number; dj: number; listening: number }[];
  confidence: number;
  confidenceLabel: 'High' | 'Medium' | 'Low';
  confidenceReasons: string[];
  verdict: string;
  flags: string[];
  findings: Finding[];
}

/** Visualization data; byte arrays are base64 encoded. */
export interface Visuals {
  waveform: {
    bucketSecs: number;
    count: number;
    /** Mono min/max, bytes mapping -1…1 to 0…255. */
    min: string;
    max: string;
    /** RMS and band RMS, bytes mapping -90…0 dBFS to 0…255. */
    rms: string;
    low: string;
    mid: string;
    high: string;
  };
  spectrogram: {
    cols: number;
    rows: number;
    secsPerCol: number;
    maxHz: number;
    dbMin: number;
    dbMax: number;
    /** Column-major [col][row], row 0 = 0 Hz, linear frequency. */
    data: string;
  };
  spectrum: { hz: number[]; mid: number[]; side: number[] | null };
  /**
   * Value `i` is the loudest `window`-second loudness window starting within
   * `[i * step, (i + 1) * step)`. `window` is missing in analyses stored by
   * early versions (3 s, or 0.4 s for files shorter than 3 s).
   */
  loudness: { step: number; window?: number; values: (number | null)[] };
  clipTimes: number[];
  overTimes: number[];
  silentRegions: [number, number][];
}

export interface StoredAnalysis {
  facts: FileFacts;
  measurements: Measurements;
  assessment: Assessment;
  visuals: Visuals | null;
  createdAt: number;
}

export interface TrackDetail {
  track: TrackRow;
  analysis: StoredAnalysis | null;
  history: { scanId: number; kind: string; startedAt: number; outcome: string; finishedAt: number | null }[];
  fileExists: boolean;
  /** Where the file came from, when it arrived through a download client's folder. */
  origin: TrackOrigin | null;
}

/** A track that arrived through the person's own Soulseek client. */
export interface TrackOrigin {
  source: 'soulseek';
  /** The file's name when it arrived. */
  originalName: string;
  /** When SONORA found it finished. */
  foundAt: number;
  /** When it joined the library. */
  addedAt: number | null;
}

/** How many downloads are in each state. */
export interface SourceCounts {
  review: number;
  imported: number;
  failed: number;
  declined: number;
  importing: number;
}

/** The download folder watcher (`sources.status`). */
export interface SourcesStatus {
  source: 'soulseek';
  /** The lawful-use notice was accepted. */
  accepted: boolean;
  acceptedAt: number | null;
  folder: string | null;
  review: boolean;
  /** Accepted, and the folder is there. */
  watching: boolean;
  lastCheck: number | null;
  error: string | null;
  /** Files seen at the last look that are not finished yet. */
  settling: number;
  /** Audio files that were in the folder when it was chosen and are not in the library: they stay out unless added. */
  existing: number;
  counts: SourceCounts;
}

/** A download held for the person to add or keep out. */
export interface SourceFile {
  path: string;
  name: string;
  size: number;
  foundAt: number;
}

// ---------------------------------------------------------------- slskd

/** The connection to slskd, the Soulseek client SONORA searches through (`slskd.status`). */
export interface SlskdStatus {
  /** slskd's web address. */
  url: string;
  /** The address is this computer. */
  local: boolean;
  keySaved: boolean;
  /** Reading the key from the system's store failed. */
  keyError: string | null;
  /** Where the key is kept. */
  keyStore: 'keychain' | 'credential-manager' | 'file';
  /** slskd's usual download folders that exist on this computer. */
  defaultFolders: string[];
}

export type SetupStepId = 'download' | 'verify' | 'install' | 'configure' | 'start' | 'signin';
export type SetupFix = 'retry' | 'password' | 'stop_other' | 'quit_soulseek_qt' | 'network' | 'folder' | 'gatekeeper';

/** Where SONORA's setup of slskd stands (`slskd.setup` events). */
export interface SetupRun {
  active: boolean;
  finished: boolean;
  steps: { id: SetupStepId; state: 'waiting' | 'running' | 'done' | 'failed' }[];
  /** Bytes of slskd downloaded so far, of `total`. */
  downloaded: number;
  total: number;
  error: { step: SetupStepId; message: string; fix: SetupFix } | null;
  /** Files and folders this run made (removed only when the person asks). */
  created: string[];
  /** The password is still in memory, so "Try again" needs no new one. */
  passwordKept: boolean;
  signedInAs: string | null;
  startedAt: number | null;
}

export interface SlskdProcess {
  pid: number;
  path: string;
}

/** slskd set up and run by SONORA (`slskd.managed`, macOS). */
export interface ManagedSlskd {
  /** SONORA can set slskd up on this computer. */
  supported: boolean;
  /** The slskd version this SONORA sets up. */
  version: string;
  /** The version unpacked, if any. */
  installed: string | null;
  updateAvailable: boolean;
  /** SONORA set up slskd here. */
  configured: boolean;
  folder: string;
  config: string | null;
  /** slskd's newest log file (or its log folder). */
  log: string | null;
  background: boolean;
  /** The person stopped it: it stays stopped (at login too) until started. */
  stopped: boolean;
  agent: { installed: boolean; loaded: boolean; running: boolean; pid: number | null; lastExit: number | null };
  username: string | null;
  downloads: string | null;
  shares: string[];
  /** slskd.yml has no key for SONORA, or another one. */
  keyProblem: 'missing' | 'different' | 'not_saved' | null;
  /** Why slskd is not signed in, from its log. */
  reason: { code: 'no_account' | 'wrong_password' | 'signed_in_elsewhere' | 'network'; message: string } | null;
  /** slskd processes other than SONORA's. */
  others: SlskdProcess[];
  /** The Soulseek app is open. */
  soulseekQt: boolean;
  setup: SetupRun | null;
}

/** What the setup screen offers (`slskd.setupPlan`). */
export interface SetupPlan {
  supported: boolean;
  version: string;
  download: { url: string; size: number; runtime: string; sha256: string } | null;
  folder: string;
  /** The launch agent's file. */
  agent: string | null;
  /** slskd settings found on this computer (the password itself never leaves the core). */
  found: { path: string; username: string | null; hasPassword: boolean; downloads: string | null; shares: string[] } | null;
  downloads: string;
  share: string | null;
  soulseekQt: boolean;
  others: SlskdProcess[];
  /** Folders macOS asks about before a program may use them. */
  protected: string[];
  /** The person's home folder. */
  home: string;
}

/** What slskd says about itself (`slskd.test`). */
export interface SlskdInfo {
  version: string;
  /** The Soulseek user name slskd signs in with. */
  username: string | null;
  connected: boolean;
  loggedIn: boolean;
  sharedFolders: number | null;
  sharedFiles: number | null;
}

/** One file in search results; format details are what the other user's client reports. */
export interface FoundFile {
  /** The full path on the other user's computer. */
  filename: string;
  name: string;
  ext: string;
  size: number;
  bitDepth: number | null;
  sampleRate: number | null;
  bitRate: number | null;
  /** Seconds. */
  length: number | null;
  lossless: boolean;
  /** A track with the same name and size is in the library. */
  inLibrary: boolean;
}

/** One folder of one user's results. */
export interface FoundFolder {
  username: string;
  directory: string;
  name: string;
  freeSlot: boolean;
  /** Downloads waiting in the user's queue. */
  queue: number;
  /** Bytes a second, as the user's client reports. */
  speed: number;
  size: number;
  /** Files left out by the format filter. */
  hidden: number;
  files: FoundFile[];
}

export interface SearchResults {
  id: string;
  text: string;
  state: 'searching' | 'done' | 'stopped' | 'failed';
  /** Users who answered. */
  responses: number;
  folders: FoundFolder[];
  /** Folders beyond the ones listed. */
  moreFolders: number;
  hiddenFiles: number;
}

/** A Soulseek file being listened to before it is downloaded (`preview.*`). */
export interface PreviewSession {
  id: number;
  username: string;
  filename: string;
  name: string;
  size: number;
  /** For the person: connecting, queued, buffering, ready (playable), reconnecting, unavailable, failed, stopped. */
  state: 'connecting' | 'queued' | 'buffering' | 'ready' | 'reconnecting' | 'unavailable' | 'failed' | 'stopped';
  /** The transfer behind it: paused means enough is read ahead. */
  transfer: 'connecting' | 'queued' | 'downloading' | 'paused' | 'complete' | 'reconnecting' | 'failed' | 'stopped';
  /** Enough has arrived to start playing. */
  ready: boolean;
  /** Place in the other user's queue. */
  queue: number | null;
  /** Bytes of the file on this computer so far. */
  received: number;
  /** How much can be played so far. */
  decodedSeconds: number | null;
  totalSeconds: number | null;
  rate: number;
  channels: number;
  /** A download of the same file was already under way: the preview listens along. */
  followsDownload: boolean;
  /** Download was chosen while listening: the download carries on from what the preview fetched. */
  handedOver: boolean;
  /** The preview cache is full: the preview stops here. */
  limitReached: boolean;
  /** No more of the file will come (the transfer was stopped elsewhere, say). */
  endsEarly: boolean;
  error: string | null;
}

export type TransferState = 'requested' | 'queued' | 'waiting' | 'starting' | 'downloading' | 'done' | 'failed' | 'cancelled';

/** One of slskd's downloads (`slskd.downloads`). */
export interface Transfer {
  id: string;
  username: string;
  filename: string;
  name: string;
  folder: string;
  size: number;
  state: TransferState;
  /** Why it failed. */
  detail: string | null;
  bytes: number;
  /** Bytes a second. */
  speed: number;
  /** Place in the other user's queue. */
  position: number | null;
  requestedAt: number | null;
  endedAt: number | null;
  /** A finished download as SONORA found it in the download folder. */
  local: { state: 'review' | 'importing' | 'imported' | 'failed' | 'declined'; path: string; error: string | null; track: TrackRow | null } | null;
}

export interface DownloadList {
  downloads: Transfer[];
  /** Downloads not finished yet. */
  active: number;
  /** Older downloads not listed. */
  more: number;
}

// ----------------------------------------------------------- duplicates

export type DuplicateKind = 'exact' | 'same_audio' | 'same_recording' | 'alternate';

export interface DuplicateMember extends TrackRow {
  match?: { with: number; ber: number; offsetSecs: number; matchedSecs: number };
}

export interface DuplicateGroup {
  id: string;
  kind: DuplicateKind;
  label: string;
  explanation: string;
  similarity: number;
  suggestedKeep: number | null;
  members: DuplicateMember[];
}

export interface DuplicateList {
  groups: DuplicateGroup[];
  fingerprintScannedAt: number | null;
  analyzedSinceScan: number;
  scanning: boolean;
}

export interface ScanItemRow extends TrackRow {
  outcome: string;
  finishedAt: number | null;
}

export type ScanItemFilter = 'all' | 'problems' | 'failed' | 'clipping' | 'source';

// ------------------------------------------------------------ DJ export

export type ExportFormat = 'aiff' | 'wav' | 'mp3';
export type ExportQuality = 'universal' | 'high';

export interface ExportOptions {
  format: ExportFormat;
  quality: ExportQuality;
  keyNotation: KeyNotation;
  bestCopy: boolean;
  rekordboxXml: boolean;
}

export interface ExportWarning {
  code: string;
  message: string;
}

export interface ExportPlanItem {
  trackId: number;
  title: string;
  artist: string | null;
  sourceId: number;
  sourcePath: string;
  sourceFormat: string | null;
  betterCopy: boolean;
  fileName: string;
  warnings: ExportWarning[];
  skip: 'missing' | 'duplicate' | null;
}

export interface ExportPlan {
  items: ExportPlanItem[];
  exported: number;
  skipped: number;
  betterCopies: number;
  withWarnings: number;
  /** Set when the export cannot run as asked (e.g. no MP3 encoder). */
  problem: string | null;
  running: boolean;
}

export interface ExportReport {
  folder: string;
  written: { trackId: number; path: string; title: string; method: 'native' | 'converted' | 'encoded' | 'copied'; bytes: number; warnings: ExportWarning[] }[];
  failed: { trackId: number; title: string; error: string }[];
  skipped: { trackId: number; title: string; reason: string }[];
  cancelled: boolean;
  rekordboxXml: string | null;
  xmlError: string | null;
  elapsedMs: number;
  error?: string;
}

export interface ExportProgress {
  done: number;
  total: number;
  fraction: number;
  current: string;
}

// ------------------------------------------------------------ playlists

export type PlaylistKind = 'folder' | 'playlist' | 'smart';

/** What a smart playlist lists: a library filter and its order. */
export interface SmartRules {
  filter: LibraryFilter;
  sort: Sort;
  limit?: number | null;
}

export interface PlaylistNode {
  id: number;
  parentId: number | null;
  kind: PlaylistKind;
  name: string;
  position: number;
  rules: SmartRules | null;
  /** Tracks (folders: everything under them, a track in two playlists twice). */
  count: number;
  /** Seconds. */
  duration: number;
  /** Folders: playlists and folders directly inside. */
  children: number;
  /** A smart playlist whose rules this version cannot read. */
  problem: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface MixSummary {
  transitions: number;
  harmonic: number;
  clashes: number;
  unknown: number;
  /** Mean tempo change between neighbours, in percent. */
  tempoChange: number | null;
}

export interface ArrangeResult {
  order: number[];
  before: MixSummary;
  after: MixSummary;
}

export type OrganizeBy = 'genre' | 'key' | 'tempo';

export interface OrganizeResult {
  folderId: number;
  playlists: number;
  tracks: number;
  without: number;
  /** Groups (genres) past the most one folder holds, left out. */
  leftOut: number;
  /** Tracks left out because their file is missing. */
  missing: number;
}

// ---------------------------------------------------------------- USB

export interface Drive {
  path: string;
  name: string;
  fs: string | null;
  fsLabel: string | null;
  total: number | null;
  free: number | null;
  removable: boolean;
}

export interface UsbOptions extends ExportOptions {
  m3u: boolean;
  serato: boolean;
  removeStale: boolean;
  numberFolders: boolean;
}

export type UsbAction = 'new' | 'update' | 'rename' | 'keep';

export interface UsbPlan {
  root: string;
  folder: string;
  playlists: { id: number; name: string; kind: PlaylistKind; dir: string; tracks: number; skipped: number; bytes: number }[];
  files: { total: number; new: number; update: number; rename: number; keep: number; stale: number; staleBytes: number };
  preview: { path: string; action: UsbAction; from: string | null; trackId: number; warnings: ExportWarning[] }[];
  bytes: { write: number; freed: number; free: number | null; total: number | null };
  fs: string | null;
  fsLabel: string | null;
  seratoPossible: boolean;
  problem: string | null;
  warnings: string[];
  running: boolean;
}

export interface UsbProgress {
  done: number;
  total: number;
  fraction: number;
  current: string;
  phase: 'preparing' | 'writing' | 'done';
}

export interface UsbReport {
  root: string;
  folder: string;
  playlists: number;
  written: number;
  renamed: number;
  kept: number;
  removed: number;
  bytes: number;
  failed: { trackId: number; title: string; error: string }[];
  m3u: number;
  crates: number;
  rekordboxXml: string | null;
  warnings: string[];
  cancelled: boolean;
  elapsedMs: number;
  error?: string;
}

// -------------------------------------------------------------- Discogs

export interface DiscogsReport {
  total: number;
  found: number;
  notFound: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
  error: string | null;
}

export interface DiscogsStatus {
  configured: boolean;
  running: boolean;
  last: DiscogsReport | null;
}

// ------------------------------------------------------------- playback

export interface PreparedPlayback {
  trackId: number;
  path: string;
  mime: string;
  transcoded: boolean;
  duration: number | null;
  /** Desktop app: the id the shell serves this one file under. */
  stream?: string;
}

// ---------------------------------------------------------------- events

export interface CoreEvents {
  'scan.started': { scan: ScanStatus };
  'scan.progress': { scan: ScanStatus };
  'scan.item': ScanItemEvent;
  'scan.finished': { scan: ScanStatus };
  'library.indexed': ImportReport | { error: string };
  'library.changed': { reason: string; count?: number };
  'duplicates.progress': { progress: number };
  /** `error` when the comparison could not be saved or failed. */
  'duplicates.updated': { error?: string };
  'djexport.progress': ExportProgress;
  'djexport.finished': ExportReport;
  'discogs.progress': { done: number; total: number; found: number; current: string };
  'discogs.finished': DiscogsReport & { error?: string | null };
  'playlists.changed': { reason: string; id?: number };
  'usb.progress': UsbProgress;
  'usb.finished': UsbReport;
  'sources.changed': SourceCounts | null;
  /** An import of downloads finished; `first` = the first ever. */
  'sources.imported': { source: 'soulseek'; added: number; failed: number; first: boolean };
  /** SONORA's setup of slskd moved on (or stopped). */
  'slskd.setup': SetupRun | null;
  /** A preview's transfer or decoding moved on. */
  'preview.changed': PreviewSession;
  /** A playlist run moved on (while a list is read: only `id`, `state` and `videos`). */
  'playlist.changed': PlaylistRun | { id: number; state: 'fetching'; videos: number };
  /** One track of a playlist run changed. */
  'playlist.item': PlaylistItem;
  'playlist.finished': PlaylistRun;
}

export type CoreEventName = keyof CoreEvents;

/** What moving tracks' files to the Trash did (`tracks.trash`). */
export interface TrashResult {
  /** Files moved to the Trash. */
  trashed: number;
  /** Tracks removed from the library (including those whose files were already gone). */
  removed: number;
  /** Tracks whose files stayed where they are, and why. */
  failed: { id: number; title: string; error: string }[];
  /** Tracks left alone because they are being analyzed right now. */
  busy: number;
  /** "Trash" or "Recycle Bin". */
  trash: string;
}
