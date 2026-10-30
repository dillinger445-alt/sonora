/**
 * Filtering the library by when tracks were added to SONORA: a period that
 * moves with the clock (the core works it out in the computer's time zone,
 * so a smart playlist of "the last 7 days" keeps up), or a range of days.
 * Free of React, so it can be tested on its own.
 */

import type { AddedPeriod, LibraryFilter } from '../api/types.ts';
import { formatDay } from './format.ts';

export const ADDED_PERIODS: { id: AddedPeriod; label: string; short: string }[] = [
  { id: 'today', label: 'Today', short: 'Today' },
  { id: '7d', label: 'Last 7 days', short: '7 days' },
  { id: '30d', label: 'Last 30 days', short: '30 days' },
  { id: '90d', label: 'Last 90 days', short: '90 days' },
  { id: 'year', label: 'This year', short: 'This year' },
];

/** What the Added filter is set to: a period, or days typed as YYYY-MM-DD (either end may be open). */
export interface AddedSelection {
  period: AddedPeriod | null;
  from: string | null;
  to: string | null;
}

export const NO_ADDED: AddedSelection = { period: null, from: null, to: null };

/** Midnight at the start of a YYYY-MM-DD day, in this computer's time zone (ms), or null. */
export function dayStart(day: string | null): number | null {
  const m = day ? /^(\d{4})-(\d{2})-(\d{2})$/.exec(day) : null;
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return Number.isNaN(d.getTime()) || d.getMonth() !== Number(m[2]) - 1 ? null : d.getTime();
}

/** The midnight after a YYYY-MM-DD day (the end of the range is included). */
function dayEnd(day: string | null): number | null {
  const start = dayStart(day);
  if (start == null) return null;
  const d = new Date(start);
  d.setDate(d.getDate() + 1);
  return d.getTime();
}

export function addedActive(s: AddedSelection): boolean {
  return s.period != null || dayStart(s.from) != null || dayStart(s.to) != null;
}

/** The filter fields for a selection. */
export function addedFilter(s: AddedSelection, now = new Date()): Pick<LibraryFilter, 'addedPeriod' | 'addedAfter' | 'addedBefore' | 'utcOffsetMin'> {
  if (s.period) return { addedPeriod: s.period, utcOffsetMin: -now.getTimezoneOffset() };
  const out: Pick<LibraryFilter, 'addedAfter' | 'addedBefore'> = {};
  const after = dayStart(s.from);
  const before = dayEnd(s.to);
  if (after != null) out.addedAfter = after;
  if (before != null) out.addedBefore = before;
  return out;
}

/** The selection a saved filter holds (to show it again). */
export function addedSelectionOf(f: LibraryFilter): AddedSelection {
  const day = (ms: number | undefined, back = 0) => {
    if (ms == null) return null;
    const d = new Date(ms - back);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  };
  return { period: f.addedPeriod ?? null, from: day(f.addedAfter), to: day(f.addedBefore, 1) };
}

/** The filter button's short text: "7 days", "Oct 1–20 Oct", "since Oct 1". */
export function addedChip(s: AddedSelection, now = Date.now()): string | null {
  if (s.period) return ADDED_PERIODS.find((p) => p.id === s.period)?.short ?? null;
  const from = dayStart(s.from);
  const to = dayStart(s.to);
  if (from != null && to != null) return from === to ? formatDay(from, now) : `${formatDay(from, now)} – ${formatDay(to, now)}`;
  if (from != null) return `since ${formatDay(from, now)}`;
  if (to != null) return `until ${formatDay(to, now)}`;
  return null;
}

/** A saved filter's dates in words, for smart playlists: "added in the last 7 days". */
export function addedWords(f: LibraryFilter, now = Date.now()): string | null {
  if (f.addedPeriod) {
    const p = ADDED_PERIODS.find((x) => x.id === f.addedPeriod);
    return p ? (f.addedPeriod === 'today' || f.addedPeriod === 'year' ? `added ${p.label.toLowerCase()}` : `added in the ${p.label.toLowerCase()}`) : null;
  }
  const chip = addedChip(addedSelectionOf(f), now);
  return chip ? `added ${/^(since|until)/.test(chip) ? chip : chip.includes('–') ? `${chip.replace(' – ', ' to ')}` : `on ${chip}`}` : null;
}
