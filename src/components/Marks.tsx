/** Score meters and the small marks used across lists and findings. */

import { LuCircleAlert, LuCircleDashed, LuCircleX, LuFileQuestion, LuInfo, LuLoaderCircle, LuOctagonAlert, LuTriangleAlert } from 'react-icons/lu';
import type { Evidence, Severity, TrackRow } from '../api/types.ts';
import { DASH } from '../lib/format.ts';
import { EVIDENCE, SEVERITY, displayFlags, flagInfo, scoreBand } from '../lib/scores.ts';

const SEGMENTS = 20;

/**
 * Segmented ladder meter for a 0–100 score, lit in the colour of the
 * score's band (lime good, amber fair, red poor).
 */
export function ScoreMeter({
  score,
  label,
  size = 'lg',
  hint,
}: {
  score: number | null | undefined;
  label: string;
  size?: 'lg' | 'md';
  hint?: string;
}) {
  const band = scoreBand(score);
  const lit = score == null ? 0 : Math.round((Math.max(0, Math.min(100, score)) / 100) * SEGMENTS);
  const aria = score == null ? `${label} score: not available` : `${label} score: ${Math.round(score)} out of 100, ${band?.label ?? ''}`;
  return (
    <div className={`meter meter-${size} ${band ? `band-${band.band}` : 'band-none'}`} role="img" aria-label={aria} data-tip={hint}>
      <div className="meter-top">
        <span className="meter-label">{label}</span>
        <span className="meter-band">{band?.label ?? 'Not scored'}</span>
      </div>
      <div className="meter-value num">{score == null ? DASH : Math.round(score)}</div>
      <div className="meter-ladder" aria-hidden>
        {Array.from({ length: SEGMENTS }, (_, i) => (
          <span key={i} className={i < lit ? 'seg is-lit' : 'seg'} />
        ))}
      </div>
    </div>
  );
}

/** Compact score for tables: number plus a 10-segment mini ladder. */
export function ScoreCell({ score, emphasis = false }: { score: number | null | undefined; emphasis?: boolean }) {
  if (score == null) return <span className="score-cell is-empty faint">{DASH}</span>;
  const band = scoreBand(score);
  const lit = Math.round(Math.max(0, Math.min(100, score)) / 10);
  return (
    <span className={`score-cell band-${band?.band ?? 'none'} ${emphasis ? 'is-emphasis' : ''}`}>
      <span className="num score-num">{Math.round(score)}</span>
      {emphasis ? (
        <span className="mini-ladder" aria-hidden>
          {Array.from({ length: 10 }, (_, i) => (
            <span key={i} className={i < lit ? 'seg is-lit' : 'seg'} />
          ))}
        </span>
      ) : null}
    </span>
  );
}

/** Evidence strength as four pips plus a label. */
export function EvidenceMark({ evidence, withLabel = true }: { evidence: Evidence; withLabel?: boolean }) {
  const info = EVIDENCE[evidence];
  return (
    <span className={`evidence evidence-${evidence}`} data-tip={`${info.label}: ${info.description}`} aria-label={`Evidence: ${info.label}`} role="img">
      <span className="pips" aria-hidden>
        {[1, 2, 3, 4].map((i) => (
          <span key={i} className={i <= info.level ? 'pip is-on' : 'pip'} />
        ))}
      </span>
      {withLabel ? <span className="evidence-label">{info.label}</span> : null}
    </span>
  );
}

const SEVERITY_ICON = {
  info: LuInfo,
  low: LuInfo,
  moderate: LuTriangleAlert,
  high: LuCircleAlert,
  critical: LuOctagonAlert,
};

export function SeverityIcon({ severity }: { severity: Severity }) {
  const Icon = SEVERITY_ICON[severity];
  const info = SEVERITY[severity];
  return <Icon className={`sev-icon sev-${info.tone}`} aria-label={info.label} role="img" />;
}

export function FlagChip({ id }: { id: string }) {
  const info = flagInfo(id);
  return (
    <span className={`chip chip-${info.tone}`} data-tip={info.description}>
      {info.label}
    </span>
  );
}

/** Up to `max` issue chips, then "+n". */
export function FlagList({ flags, max = 2 }: { flags: string[]; max?: number }) {
  const list = displayFlags(flags);
  if (list.length === 0) return null;
  const shown = list.slice(0, max);
  const rest = list.slice(max);
  return (
    <span className="flag-list">
      {shown.map((f) => (
        <FlagChip key={f} id={f} />
      ))}
      {rest.length > 0 ? (
        <span className="chip chip-more" data-tip={rest.map((f) => flagInfo(f).label).join(', ')}>
          +{rest.length}
        </span>
      ) : null}
    </span>
  );
}

/** Analysis state of a track when it is not simply "done". */
export function StatusMark({ track, compact = false }: { track: Pick<TrackRow, 'status' | 'missing' | 'errorMessage'>; compact?: boolean }) {
  if (track.missing) {
    return (
      <span className="status status-missing" data-tip="The file is no longer at this location. Its results are kept; rescanning its folder relinks moved files.">
        <LuFileQuestion aria-hidden />
        {compact ? <span className="visually-hidden">Missing</span> : 'Missing'}
      </span>
    );
  }
  switch (track.status) {
    case 'pending':
      return (
        <span className="status status-pending" data-tip="Waiting to be analyzed">
          <LuCircleDashed aria-hidden />
          {compact ? <span className="visually-hidden">Pending</span> : 'Pending'}
        </span>
      );
    case 'analyzing':
      return (
        <span className="status status-analyzing">
          <LuLoaderCircle className="spin" aria-hidden />
          {compact ? <span className="visually-hidden">Analyzing</span> : 'Analyzing'}
        </span>
      );
    case 'failed':
      return (
        <span className="status status-failed" data-tip={track.errorMessage ?? 'Analysis failed'}>
          <LuCircleX aria-hidden />
          {compact ? <span className="visually-hidden">Failed</span> : 'Failed'}
        </span>
      );
    default:
      return null;
  }
}
