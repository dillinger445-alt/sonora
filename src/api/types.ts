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
}

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
}

export interface Overview {
  tracks: { total: number; analyzed: number; pending: number; analyzing: number; failed: number; missing: number };
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
  | 'status';

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
  avgOverall: number | null;
  bytes: number;
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

// ---------------------------------------------------------------- events

export interface CoreEvents {
  'scan.started': { scan: ScanStatus };
  'scan.progress': { scan: ScanStatus };
  'scan.item': ScanItemEvent;
  'scan.finished': { scan: ScanStatus };
  'library.indexed': ImportReport | { error: string };
  'library.changed': { reason: string; count?: number };
  'duplicates.progress': { progress: number };
  'duplicates.updated': Record<string, never>;
}

export type CoreEventName = keyof CoreEvents;
