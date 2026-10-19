/** Column definitions shared by the track tables. */

import type { ReactNode } from 'react';
import type { ScoreKind, SortKey, TrackRow } from '../api/types.ts';
import { DASH, encodingShort, formatDb, formatDuration, formatRateKhz } from '../lib/format.ts';
import { confidenceTone } from '../lib/scores.ts';
import { FlagList, ScoreCell, StatusMark } from './Marks.tsx';

export type ColumnId =
  | 'title'
  | 'artist'
  | 'format'
  | 'duration'
  | 'lufs'
  | 'truePeak'
  | 'cutoff'
  | 'overall'
  | 'dj'
  | 'listening'
  | 'confidence'
  | 'issues';

export interface Column {
  id: ColumnId;
  label: string;
  /** Accessible / tooltip description of the header. */
  description?: string;
  /** CSS grid track. */
  width: string;
  /** Minimum pixels the track needs (to decide what fits). */
  min: number;
  align?: 'end';
  sortKey?: SortKey;
  /** Lower survives longer when space runs out; 0 = always shown. */
  priority: number;
  render: (t: TrackRow, scoreKind: ScoreKind) => ReactNode;
}

const kHz = (hz: number | null) => (hz == null ? DASH : formatRateKhz(hz));

export function scoreColumn(kind: ScoreKind, emphasis: boolean): Column {
  const label = kind === 'overall' ? 'Overall' : kind === 'dj' ? 'DJ' : 'Listening';
  return {
    id: kind,
    label,
    description: `${label} score (0–100)`,
    width: emphasis ? '96px' : '62px',
    min: emphasis ? 96 : 62,
    align: 'end',
    sortKey: kind,
    priority: emphasis ? 0 : 2,
    render: (t) => <ScoreCell score={t[kind]} emphasis={emphasis} />,
  };
}

export function trackColumns(scoreKind: ScoreKind): Column[] {
  return [
    {
      id: 'title',
      label: 'Title',
      width: 'minmax(180px, 2.6fr)',
      min: 200,
      sortKey: 'title',
      priority: 0,
      render: (t) => (
        <span className="cell-title">
          <StatusMark track={t} compact />
          <span className="ellipsis" title={t.displayTitle}>
            {t.displayTitle}
          </span>
        </span>
      ),
    },
    {
      id: 'artist',
      label: 'Artist',
      width: 'minmax(110px, 1.4fr)',
      min: 120,
      sortKey: 'artist',
      priority: 4,
      render: (t) => <span className="ellipsis muted">{t.artist ?? ''}</span>,
    },
    {
      id: 'format',
      label: 'Format',
      description: 'Codec, then bit depth / sample rate in kHz (lossless) or bitrate (lossy)',
      width: '112px',
      min: 112,
      sortKey: 'format',
      priority: 1,
      render: (t) =>
        t.format ? (
          <span className="cell-format">
            <span className="fmt-name">{t.format}</span>
            <span className="num faint">{encodingShort(t)}</span>
          </span>
        ) : (
          <span className="faint">{t.ext ? t.ext.toUpperCase() : DASH}</span>
        ),
    },
    {
      id: 'duration',
      label: 'Time',
      description: 'Duration',
      width: '62px',
      min: 62,
      align: 'end',
      sortKey: 'duration',
      priority: 5,
      render: (t) => <span className="num muted">{formatDuration(t.duration)}</span>,
    },
    {
      id: 'lufs',
      label: 'LUFS',
      description: 'Integrated loudness (EBU R128)',
      width: '62px',
      min: 62,
      align: 'end',
      sortKey: 'lufs',
      priority: 6,
      render: (t) => <span className="num muted">{formatDb(t.integratedLufs)}</span>,
    },
    {
      id: 'truePeak',
      label: 'dBTP',
      description: 'True peak: the highest inter-sample peak (ITU-R BS.1770)',
      width: '62px',
      min: 62,
      align: 'end',
      sortKey: 'truePeak',
      priority: 7,
      render: (t) => (
        <span className={`num ${t.truePeak != null && t.truePeak > 0 ? 't-red' : 'muted'}`}>{formatDb(t.truePeak, 1, true)}</span>
      ),
    },
    {
      id: 'cutoff',
      label: 'Cutoff',
      description: 'Steep high-frequency cutoff in kHz, when one was detected',
      width: '64px',
      min: 64,
      align: 'end',
      sortKey: 'cutoff',
      priority: 8,
      render: (t) => <span className="num muted">{kHz(t.cutoffHz)}</span>,
    },
    scoreColumn('overall', scoreKind === 'overall'),
    scoreColumn('dj', scoreKind === 'dj'),
    scoreColumn('listening', scoreKind === 'listening'),
    {
      id: 'confidence',
      label: 'Conf.',
      description: 'Confidence in the assessment',
      width: '66px',
      min: 66,
      sortKey: 'confidence',
      priority: 6,
      render: (t) =>
        t.confidenceLabel ? <span className={`conf conf-${confidenceTone(t.confidenceLabel) ?? 'none'}`}>{t.confidenceLabel}</span> : <span className="faint">{DASH}</span>,
    },
    {
      id: 'issues',
      label: 'Issues',
      width: 'minmax(132px, 1.3fr)',
      min: 140,
      priority: 3,
      render: (t) =>
        t.status === 'failed' ? (
          <span className="ellipsis t-red" title={t.errorMessage ?? ''}>
            {t.errorMessage ?? 'Analysis failed'}
          </span>
        ) : (
          <FlagList flags={t.flags} max={1} />
        ),
    },
  ];
}

const COLUMN_GAP = 10;

/** Columns that fit `width` pixels (after the 36px selection column, gaps and padding). */
export function fitColumns(columns: Column[], width: number): Column[] {
  const available = width - 36 - 8;
  const kept = [...columns];
  const total = () => kept.reduce((s, c) => s + c.min + COLUMN_GAP, 0);
  while (total() > available) {
    let drop = -1;
    let worst = 0;
    // Ties drop the rightmost column first.
    kept.forEach((c, i) => {
      if (c.priority >= worst && c.priority > 0) {
        worst = c.priority;
        drop = i;
      }
    });
    if (drop < 0) break;
    kept.splice(drop, 1);
  }
  return kept;
}
