import { LuArrowLeft, LuHistory } from 'react-icons/lu';
import type { ScanRecord } from '../api/types.ts';
import { IconButton } from '../components/Button.tsx';
import { EmptyState, ErrorState, Section, ViewHeader } from '../components/Layout.tsx';
import { ScoreCell } from '../components/Marks.tsx';
import { formatCount, formatDate, formatSpan, middleEllipsis, plural } from '../lib/format.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useResource } from '../state/useResource.ts';
import { stateLabel } from './Overview.tsx';
import { ScanResults } from './Queue.tsx';

export function kindLabel(kind: string): string {
  switch (kind) {
    case 'import':
      return 'Import';
    case 'folders':
      return 'Folder rescan';
    case 'startup':
      return 'Rescan at launch';
    case 'rescan':
      return 'Re-analysis';
    case 'retry':
      return 'Retry of failed files';
    case 'pending':
      return 'Waiting files';
    case 'resume':
      return 'Resumed after restart';
    default:
      return kind;
  }
}

function duration(s: ScanRecord): string {
  if (s.summary?.elapsedMs != null) return formatSpan(s.summary.elapsedMs / 1000);
  if (s.finishedAt) return formatSpan((s.finishedAt - s.startedAt) / 1000);
  return '—';
}

function ScanDetail({ scanId }: { scanId: number }) {
  const { client } = useCore();
  const history = useResource(() => client.history(500), [], { events: ['scan.finished'] });
  const s = history.data?.find((x) => x.id === scanId);
  return (
    <div className="view view-scroll">
      <ViewHeader
        back={<IconButton label="Back to analysis history" icon={LuArrowLeft} onClick={() => navigate({ view: 'history' })} />}
        title={s ? `${kindLabel(s.kind)}, ${formatDate(s.startedAt)}` : 'Scan'}
        subtitle={
          s?.summary?.message && s.total === 0 ? (
            <>
              {s.summary.message}
              {s.skipped ? `: ${plural(s.skipped, 'unchanged file')} skipped` : ''}.
            </>
          ) : s ? (
            <>
              {stateLabel(s.state)}: {plural(s.completed, 'track')} analyzed
              {s.failed ? `, ${formatCount(s.failed)} failed` : ''}
              {s.skipped ? `, ${formatCount(s.skipped)} skipped` : ''}
              {s.cancelled ? `, ${formatCount(s.cancelled)} not analyzed` : ''}. Took {duration(s)}.
            </>
          ) : undefined
        }
      />
      {s && s.roots.length ? (
        <Section title="Locations">
          <ul className="roots">
            {s.roots.map((r) => (
              <li key={r} className="selectable faint" title={r}>
                {middleEllipsis(r, 110)}
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
      <ScanResults scanId={scanId} live={false} />
    </div>
  );
}

export function HistoryView({ scanId }: { scanId?: number }) {
  const { client } = useCore();
  const history = useResource(() => client.history(200), [], { events: ['scan.finished', 'scan.started'] });
  if (scanId != null) return <ScanDetail scanId={scanId} />;
  const list = history.data ?? [];
  return (
    <div className="view view-scroll">
      <ViewHeader title="Analysis history" subtitle="Every scan with its results. Open one to see what it found." />
      {history.error ? <ErrorState title="Could not load the history" error={history.error} onRetry={history.reload} /> : null}
      {!history.loading && list.length === 0 ? (
        <EmptyState icon={LuHistory} title="No scans yet">
          Each import, rescan or re-analysis is recorded here with its results.
        </EmptyState>
      ) : null}
      {list.length > 0 ? (
        <table className="list-table history-table">
          <thead>
            <tr>
              <th>Started</th>
              <th>Kind</th>
              <th>State</th>
              <th className="is-end">Analyzed</th>
              <th className="is-end">Failed</th>
              <th className="is-end">Skipped</th>
              <th className="is-end">Time</th>
              <th className="is-end" data-tip="Average overall score of the tracks this scan analyzed">
                Avg.
              </th>
              <th>Locations</th>
            </tr>
          </thead>
          <tbody>
            {list.map((s) => (
              <tr key={s.id} className="is-link" onClick={() => navigate({ view: 'history', scanId: s.id })}>
                <td>
                  <a className="row-link" href={formatRoute({ view: 'history', scanId: s.id })} onClick={(e) => e.stopPropagation()}>
                    {formatDate(s.startedAt)}
                  </a>
                </td>
                <td>{kindLabel(s.kind)}</td>
                <td>
                  <span className={`scan-state state-${s.state}`}>{stateLabel(s.state)}</span>
                </td>
                <td className="is-end num">{formatCount(s.completed)}</td>
                <td className={`is-end num ${s.failed ? 't-red' : 'faint'}`}>{formatCount(s.failed)}</td>
                <td className="is-end num faint">{formatCount(s.skipped)}</td>
                <td className="is-end num muted">{duration(s)}</td>
                <td className="is-end">
                  <ScoreCell score={s.summary?.avgOverall ?? null} />
                </td>
                <td className="ellipsis faint" title={s.roots.join('\n')}>
                  {s.roots.length === 0 ? '—' : s.roots.length === 1 ? middleEllipsis(s.roots[0] ?? '', 48) : `${middleEllipsis(s.roots[0] ?? '', 36)} and ${s.roots.length - 1} more`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
