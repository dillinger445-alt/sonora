/** Formatting helpers. Missing values render as an em dash. */

export const DASH = '—';

const intFmt = new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 });

export function formatCount(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return DASH;
  return intFmt.format(n);
}

/** `seconds` as m:ss or h:mm:ss. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return DASH;
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** Long durations in words: "92 h 14 min", "14 min", "45 s". */
export function formatSpan(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return DASH;
  if (seconds < 60) return `${Math.round(seconds)} s`;
  const totalMin = Math.round(seconds / 60);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** Time position on a track: m:ss.s for short spans, m:ss otherwise. */
export function formatTimecode(seconds: number, precise = false): string {
  if (!Number.isFinite(seconds)) return DASH;
  const sign = seconds < 0 ? '-' : '';
  const t = Math.abs(seconds);
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  const sec = precise ? s.toFixed(1).padStart(4, '0') : String(Math.floor(s)).padStart(2, '0');
  return `${sign}${m}:${sec}`;
}

/** Decimal units, as Finder shows them. Rounded before the unit is
 *  chosen, so 999,700 bytes is "1.0 MB", never "1000 kB" (B56). */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return DASH;
  if (Math.round(bytes) < 1000) return `${Math.round(bytes)} B`;
  const units = ['kB', 'MB', 'GB', 'TB'];
  let v = bytes;
  let i = -1;
  const shown = (x: number) => (x >= 99.95 ? Math.round(x) : Math.round(x * 10) / 10);
  while ((v >= 1000 || shown(v) >= 1000) && i < units.length - 1) {
    v /= 1000;
    i += 1;
  }
  return `${shown(v) >= 100 ? shown(v).toFixed(0) : shown(v).toFixed(1)} ${units[i]}`;
}

/** A frequency in Hz or kHz. */
export function formatHz(hz: number | null | undefined): string {
  if (hz == null || !Number.isFinite(hz)) return DASH;
  if (hz >= 1000) return `${trimZero((hz / 1000).toFixed(1))} kHz`;
  return `${Math.round(hz)} Hz`;
}

/** Sample rate in kHz without the unit: 44100 → "44.1", 48000 → "48". */
export function formatRateKhz(rate: number | null | undefined): string {
  if (rate == null || !Number.isFinite(rate) || rate <= 0) return DASH;
  return trimZero((rate / 1000).toFixed(1));
}

/** A level in dB with one decimal; `signed` adds "+" to positive values. */
export function formatDb(v: number | null | undefined, digits = 1, signed = false): string {
  if (v == null || !Number.isFinite(v)) return DASH;
  if (v <= -199) return '-inf';
  const s = v.toFixed(digits);
  const normalized = /^-0(\.0+)?$/.test(s) ? s.slice(1) : s;
  return signed && v > 0 && Number(normalized) !== 0 ? `+${normalized}` : normalized;
}

export function formatPercent(fraction: number | null | undefined, digits = 0): string {
  if (fraction == null || !Number.isFinite(fraction)) return DASH;
  return `${(fraction * 100).toFixed(digits)}%`;
}

export function formatKbps(kbps: number | null | undefined): string {
  if (kbps == null || !Number.isFinite(kbps) || kbps <= 0) return DASH;
  return intFmt.format(Math.round(kbps));
}

function trimZero(s: string): string {
  return s.endsWith('.0') ? s.slice(0, -2) : s;
}

/** The system locale's date format, or English if the system locale is unusable. */
function dateFormat(opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  try {
    return new Intl.DateTimeFormat(undefined, opts);
  } catch {
    return new Intl.DateTimeFormat('en-US', opts);
  }
}

const dateFmt = dateFormat({ month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
const dateYearFmt = dateFormat({ year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

/** Absolute date and time ("Oct 19, 14:20"; the year only when it differs). */
export function formatDate(ms: number | null | undefined, now = Date.now()): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return DASH;
  const d = new Date(ms);
  return d.getFullYear() === new Date(now).getFullYear() ? dateFmt.format(d) : dateYearFmt.format(d);
}

const timeFmt = dateFormat({ hour: '2-digit', minute: '2-digit' });

/** Time of day for today's timestamps, the date otherwise. */
export function formatClock(ms: number | null | undefined, now = Date.now()): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return DASH;
  const d = new Date(ms);
  return d.toDateString() === new Date(now).toDateString() ? timeFmt.format(d) : formatDate(ms, now);
}

/** "just now", "4 min ago", "3 h ago", "yesterday", then the date. */
export function formatRelative(ms: number | null | undefined, now = Date.now()): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return DASH;
  const diff = Math.max(0, now - ms) / 1000;
  if (diff < 45) return 'just now';
  // Rounded first, so 59.8 minutes is "1 h ago", not "60 min ago" (B75).
  const min = Math.max(1, Math.round(diff / 60));
  if (min < 60) return `${min} min ago`;
  const h = Math.round(diff / 3600);
  if (h < 24) return `${h} h ago`;
  if (diff < 2 * 86400) return 'yesterday';
  return formatDate(ms, now);
}

/** Whole minutes split into hours and minutes after rounding (B75). */
function hoursAndMinutes(s: number): [number, number] {
  const total = Math.round(s / 60);
  return [Math.floor(total / 60), total % 60];
}

/** Remaining time: "about 4 min", "under a minute". */
export function formatEta(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return DASH;
  const s = ms / 1000;
  if (s < 60) return 'under a minute';
  const [h, m] = hoursAndMinutes(s);
  if (h === 0) return `about ${m} min`;
  return m === 0 ? `about ${h} h` : `about ${h} h ${m} min`;
}

/** Compact remaining time for tight spaces: "<1 min", "4 min", "1 h 5 min". */
export function formatEtaShort(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return DASH;
  const s = ms / 1000;
  if (s < 60) return '<1 min';
  const [h, m] = hoursAndMinutes(s);
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
}

/** Channel count as a word. */
export function formatChannels(n: number | null | undefined): string {
  if (n == null) return DASH;
  if (n === 1) return 'Mono';
  if (n === 2) return 'Stereo';
  return `${n} channels`;
}

/** Short technical descriptor of a track's encoding: "16/44.1", "320k". */
export function encodingShort(t: {
  lossless: boolean | null;
  bitDepth: number | null;
  sampleRate: number | null;
  bitrateKbps: number | null;
  format: string | null;
}): string {
  if (t.lossless === false) {
    return t.bitrateKbps ? `${Math.round(t.bitrateKbps)}k` : formatRateKhz(t.sampleRate);
  }
  const rate = formatRateKhz(t.sampleRate);
  if (t.bitDepth) return `${t.bitDepth}/${rate}`;
  return rate;
}

/** Shortens a path in the middle, keeping the file name. */
export function middleEllipsis(path: string, max = 64): string {
  if (path.length <= max) return path;
  const sep = path.includes('\\') && !path.includes('/') ? '\\' : '/';
  const parts = path.split(sep);
  const file = parts.pop() ?? '';
  if (file.length + 4 >= max) return `…${sep}${file.slice(-(max - 2))}`;
  let head = '';
  for (const p of parts) {
    if (head.length + p.length + file.length + 5 > max) break;
    head += p + sep;
  }
  return `${head}…${sep}${file}`;
}
