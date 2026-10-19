/** Right-hand panel with a quick read of the focused track. */

import { LuExternalLink, LuFolderSearch, LuRefreshCw, LuTrash2 } from 'react-icons/lu';
import type { TrackDetail, TrackRow } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { EvidenceMark, ScoreMeter, SeverityIcon, StatusMark } from '../components/Marks.tsx';
import { DASH, encodingShort, formatBytes, formatDb, formatDuration, formatHz, formatKbps, middleEllipsis, plural } from '../lib/format.ts';
import { useDebouncedValue } from '../lib/persist.ts';
import { navigate } from '../lib/router.ts';
import { SCORE_HINT, SCORE_KINDS, SCORE_LABEL, SEVERITY } from '../lib/scores.ts';
import { useCore } from '../state/core.tsx';
import type { TrackActions } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';

export function Inspector({
  row,
  selectionCount,
  actions,
  onRemove,
  onClearSelection,
  selectedIds,
}: {
  row: TrackRow | null;
  selectionCount: number;
  selectedIds: number[];
  actions: TrackActions;
  onRemove: (ids: number[], what?: string) => Promise<void>;
  onClearSelection: () => void;
}) {
  const { client } = useCore();
  // Findings load once the cursor rests on a row, without the (large) visualization data.
  const settled = useDebouncedValue(row, 120);
  const id = settled?.id ?? null;
  const analyzedAt = settled?.analyzedAt ?? null;
  const detail = useResource<TrackDetail | null>(() => (id != null ? client.track(id, { visuals: false }) : Promise.resolve(null)), [id, analyzedAt], {
    enabled: id != null,
  });

  if (selectionCount > 1) {
    return (
      <aside className="inspector" aria-label="Selection">
        <div className="insp-head">
          <h2 className="insp-title">{plural(selectionCount, 'track')} selected</h2>
          <p className="insp-sub">Actions apply to every selected track. Files on disk are never changed.</p>
        </div>
        <div className="insp-actions is-stacked">
          <Button icon={LuRefreshCw} onClick={() => void actions.reanalyze(selectedIds)}>
            Re-analyze {plural(selectionCount, 'track')}
          </Button>
          <Button onClick={() => void actions.exportCsv({ ids: selectedIds }, selectionCount)}>Export as CSV</Button>
          <Button icon={LuTrash2} variant="danger" onClick={() => void onRemove(selectedIds)}>
            Remove from library
          </Button>
          <Button variant="ghost" onClick={onClearSelection}>
            Clear selection
          </Button>
        </div>
      </aside>
    );
  }

  if (!row) {
    return (
      <aside className="inspector is-empty" aria-label="Track details">
        <p className="faint">Select a track to see its scores and main findings. Press Enter or double-click to open the full analysis.</p>
      </aside>
    );
  }

  const a = detail.data?.track.id === row.id ? detail.data.analysis : null;
  const findings = a
    ? [...a.assessment.findings]
        .filter((f) => f.severity !== 'info' || f.evidence === 'inconclusive')
        .sort((x, y) => SEVERITY[y.severity].rank - SEVERITY[x.severity].rank || y.penalty.overall - x.penalty.overall)
        .slice(0, 4)
    : [];

  return (
    <aside className="inspector" aria-label="Track details">
      <div className="insp-head">
        <h2 className="insp-title selectable">{row.displayTitle}</h2>
        <p className="insp-sub selectable">{[row.artist, row.album].filter(Boolean).join(' – ') || row.fileName}</p>
      </div>

      {row.missing || row.status !== 'done' ? (
        <div className={`insp-status ${row.status === 'failed' ? 'is-failed' : ''}`}>
          <StatusMark track={row} />
          {row.status === 'failed' ? <p className="selectable">{row.errorMessage ?? 'The file could not be analyzed.'}</p> : null}
          {row.missing ? <p>The file is no longer at its recorded location. Rescan its folder to relink a moved file.</p> : null}
          {row.status === 'pending' && !row.missing ? <p>Waiting in the analysis queue.</p> : null}
        </div>
      ) : null}

      {row.overall != null ? (
        <div className="insp-meters">
          {SCORE_KINDS.map((k) => (
            <ScoreMeter key={k} score={row[k]} label={SCORE_LABEL[k]} size="md" hint={SCORE_HINT[k]} />
          ))}
        </div>
      ) : null}

      {row.verdict ? (
        <div className="insp-verdict">
          <p className="selectable">{row.verdict}</p>
          {row.confidenceLabel ? (
            <p className="insp-conf">
              Confidence <span className={`conf conf-${row.confidenceLabel === 'High' ? 'good' : row.confidenceLabel === 'Medium' ? 'fair' : 'poor'}`}>{row.confidenceLabel}</span>
              <span className="num faint"> {row.confidence != null ? row.confidence.toFixed(2) : ''}</span>
            </p>
          ) : null}
        </div>
      ) : null}

      {findings.length > 0 ? (
        <ul className="insp-findings" aria-label="Main findings">
          {findings.map((f) => (
            <li key={f.id} className="insp-finding">
              <SeverityIcon severity={f.severity} />
              <span className="insp-finding-title">{f.title}</span>
              <EvidenceMark evidence={f.evidence} withLabel={false} />
            </li>
          ))}
        </ul>
      ) : null}

      <dl className="insp-facts">
        <div>
          <dt>Format</dt>
          <dd>
            {row.format ?? row.ext.toUpperCase()} <span className="num faint">{row.format ? encodingShort(row) : ''}</span>
          </dd>
        </div>
        <div>
          <dt>Bitrate</dt>
          <dd className="num">{row.bitrateKbps ? `${formatKbps(row.bitrateKbps)} kbps` : DASH}</dd>
        </div>
        <div>
          <dt>Duration</dt>
          <dd className="num">{formatDuration(row.duration)}</dd>
        </div>
        <div>
          <dt>Loudness</dt>
          <dd className="num">{row.integratedLufs != null ? `${formatDb(row.integratedLufs)} LUFS` : DASH}</dd>
        </div>
        <div>
          <dt>True peak</dt>
          <dd className={`num ${row.truePeak != null && row.truePeak > 0 ? 't-red' : ''}`}>{row.truePeak != null ? `${formatDb(row.truePeak, 2, true)} dBTP` : DASH}</dd>
        </div>
        <div>
          <dt>Cutoff</dt>
          <dd className="num">{row.cutoffHz ? formatHz(row.cutoffHz) : 'None detected'}</dd>
        </div>
        <div>
          <dt>Size</dt>
          <dd className="num">{formatBytes(row.size)}</dd>
        </div>
        <div className="is-wide">
          <dt>Location</dt>
          <dd className="selectable insp-path" title={row.path}>
            {middleEllipsis(row.path, 72)}
          </dd>
        </div>
      </dl>

      <div className="insp-actions">
        <Button variant="primary" icon={LuExternalLink} onClick={() => navigate({ view: 'track', id: row.id })} disabled={row.status !== 'done'}>
          Open analysis
        </Button>
        <Button icon={LuFolderSearch} onClick={() => void actions.reveal(row.path)} disabled={row.missing}>
          {actions.revealText}
        </Button>
        <Button icon={LuRefreshCw} onClick={() => void actions.reanalyze([row.id])} disabled={row.missing || row.status === 'analyzing'}>
          Re-analyze
        </Button>
        <Button
          icon={LuTrash2}
          variant="ghost"
          className="is-danger-text"
          onClick={() => void onRemove([row.id], `“${row.displayTitle}”`)}
        >
          Remove from library
        </Button>
      </div>
    </aside>
  );
}
