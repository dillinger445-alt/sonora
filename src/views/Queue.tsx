/** Import queue: live scan progress, controls and results as they arrive. */

import { memo, useId, useMemo, useRef, useState } from 'react';
import { LuFolderPlus, LuInbox, LuPause, LuPlay, LuPlus, LuRotateCw, LuSquare } from 'react-icons/lu';
import type { QueueResult } from '../api/client.ts';
import type { ScanItemFilter, ScanItemRow, ScanRecord, ScanStatus, ScanSummary } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { ProgressBar, TabList, tabId } from '../components/Controls.tsx';
import { EmptyState, ErrorState, Section, ViewHeader } from '../components/Layout.tsx';
import { FlagList, ScoreCell } from '../components/Marks.tsx';
import { encodingShort, formatClock, formatCount, formatDate, formatEtaShort, formatPercent, formatSpan, formatTimecode, plural } from '../lib/format.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { reuseRows } from '../lib/rows.ts';
import { useCore, useScan } from '../state/core.tsx';
import { useImports } from '../state/imports.tsx';
import { useToast } from '../state/toasts.tsx';
import { describeQueued } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';
import { stateLabel } from './Overview.tsx';

const FILTERS: { id: ScanItemFilter; label: string }[] = [
  { id: 'all', label: 'All results' },
  { id: 'problems', label: 'Problems' },
  { id: 'failed', label: 'Failed' },
  { id: 'clipping', label: 'Clipping' },
  { id: 'source', label: 'Lossy source' },
];

function Counter({ label, value, tone }: { label: string; value: number; tone?: 'red' | 'lime' | 'amber' }) {
  return (
    <div className="counter">
      <dt>{label}</dt>
      <dd className={`num ${tone && value > 0 ? `t-${tone}` : ''}`}>{formatCount(value)}</dd>
    </div>
  );
}

function LiveScan({ scan }: { scan: ScanStatus }) {
  const done = scan.done;
  const fraction = scan.total > 0 ? Math.min(1, done / scan.total) : 0;
  if (scan.state === 'indexing') {
    return (
      <div className="live">
        <div className="live-indexing">
          <span className="sb-dot is-indexing" aria-hidden />
          <div>
            <div className="live-title">Looking for audio files</div>
            <div className="faint selectable">{scan.indexing ?? ''}</div>
          </div>
        </div>
        <p className="faint">Files are listed first; analysis starts as soon as indexing finishes. Unchanged files are skipped.</p>
      </div>
    );
  }
  return (
    <div className="live">
      <div className="live-head">
        <div className="live-title">
          {scan.state === 'paused' ? 'Paused' : scan.state === 'cancelling' ? 'Cancelling' : 'Analyzing'}
          <span className="num live-fraction">
            {formatCount(done)} of {formatCount(scan.total)}
          </span>
        </div>
        <span className="num muted">{formatPercent(fraction)}</span>
      </div>
      <ProgressBar value={fraction} label="Scan progress" tone={scan.state === 'paused' ? 'amber' : 'lime'} />
      <dl className="counters">
        <Counter label="Analyzed" value={scan.completed} />
        <Counter label="Failed" value={scan.failed} tone="red" />
        <Counter label="Skipped" value={scan.skipped} />
        <Counter label="Waiting" value={scan.pending} />
        <Counter label="In progress" value={scan.running} tone="lime" />
        <div className="counter">
          <dt>Elapsed</dt>
          <dd className="num">{formatTimecode(scan.elapsedMs / 1000)}</dd>
        </div>
        <div className="counter">
          <dt>Remaining</dt>
          <dd className="num">{scan.state === 'running' && scan.etaMs != null ? formatEtaShort(scan.etaMs) : '—'}</dd>
        </div>
        <div className="counter">
          <dt>Rate</dt>
          <dd className="num">{scan.ratePerMin != null ? `${scan.ratePerMin.toFixed(1)}/min` : '—'}</dd>
        </div>
      </dl>
      {scan.current.length > 0 ? (
        <ul className="workers" aria-label="Files being analyzed">
          {scan.current.map((w) => (
            <li key={w.trackId} className="worker">
              <span className="ellipsis" title={w.path}>
                {w.fileName}
              </span>
              <ProgressBar value={w.progress} label={`Progress of ${w.fileName}`} size="sm" tone={scan.state === 'paused' ? 'amber' : 'lime'} />
              <span className="num faint">{formatTimecode(w.elapsedMs / 1000)}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function Summary({ summary, record }: { summary: ScanSummary | null | undefined; record: ScanRecord | ScanStatus }) {
  const s = summary;
  if (s?.message && record.total === 0) {
    return (
      <div className="summary">
        <div className="summary-line">
          <strong>{s.message}</strong>
          <span className="muted">{record.skipped ? `: ${plural(record.skipped, 'unchanged file')} skipped.` : '.'}</span>
        </div>
      </div>
    );
  }
  return (
    <div className="summary">
      <div className="summary-line">
        <strong>{stateLabel(record.state)}</strong>
        {s && s.elapsedMs > 0 ? <span className="muted"> in {formatSpan(s.elapsedMs / 1000)}</span> : null}
        <span className="muted">
          : {plural(record.completed, 'track')} analyzed
          {record.failed ? `, ${formatCount(record.failed)} failed` : ''}
          {record.skipped ? `, ${formatCount(record.skipped)} skipped` : ''}
          {'cancelled' in record && record.cancelled ? `, ${formatCount(record.cancelled)} not analyzed` : ''}.
        </span>
      </div>
      {s && record.completed > 0 ? (
        <dl className="counters">
          <div className="counter">
            <dt>Overall average</dt>
            <dd>
              <ScoreCell score={s.avgOverall} emphasis />
            </dd>
          </div>
          <div className="counter">
            <dt>DJ average</dt>
            <dd>
              <ScoreCell score={s.avgDj} emphasis />
            </dd>
          </div>
          <div className="counter">
            <dt>Listening average</dt>
            <dd>
              <ScoreCell score={s.avgListening} emphasis />
            </dd>
          </div>
          <Counter label="Need attention" value={s.attention} tone="amber" />
          <Counter label="Clipping" value={s.clipping} tone="red" />
          <Counter label="Lossy source" value={s.sourceConcerns} tone="amber" />
          <Counter label="Integrity" value={s.integrity} tone="red" />
          <div className="counter">
            <dt>Audio</dt>
            <dd className="num">{s.audioSeconds != null ? formatSpan(s.audioSeconds) : '—'}</dd>
          </div>
        </dl>
      ) : null}
    </div>
  );
}

/** Results shown at a time. */
const RESULTS_PAGE = 500;

/**
 * One result. Memoised: while a scan runs the results are fetched again
 * about once a second, and only rows that changed (new results) render.
 */
const ResultRow = memo(function ResultRow({ t }: { t: ScanItemRow }) {
  return (
    <tr className={`is-link ${t.outcome === 'failed' ? 'is-failed' : ''}`} onClick={() => t.status === 'done' && navigate({ view: 'track', id: t.id })}>
      <td className="cell-title-2">
        {t.status === 'done' ? (
          <a className="ellipsis row-link" href={formatRoute({ view: 'track', id: t.id })} onClick={(e) => e.stopPropagation()}>
            {t.displayTitle}
          </a>
        ) : (
          <span className="ellipsis">{t.displayTitle}</span>
        )}
        <span className="ellipsis faint">{t.artist ?? t.fileName}</span>
      </td>
      <td>
        {t.format ?? t.ext.toUpperCase()} <span className="num faint">{t.format ? encodingShort(t) : ''}</span>
      </td>
      <td className="is-end">
        <ScoreCell score={t.overall} emphasis />
      </td>
      <td className="is-end">
        <ScoreCell score={t.dj} />
      </td>
      <td className="is-end">
        <ScoreCell score={t.listening} />
      </td>
      <td className="cell-issues">
        {t.outcome === 'failed' ? (
          <span className="t-red ellipsis" title={t.errorMessage ?? ''}>
            {t.errorMessage ?? 'Failed'}
          </span>
        ) : (
          <FlagList flags={t.flags} />
        )}
      </td>
      <td className="is-end faint">{t.finishedAt ? formatClock(t.finishedAt) : ''}</td>
    </tr>
  );
});

export function ScanResults({ scanId, live }: { scanId: number; live: boolean }) {
  const { client } = useCore();
  const idBase = useId();
  const [filter, setFilter] = useState<ScanItemFilter>('all');
  // How many results to show; Analysis history can show them all (B28).
  const [shown, setShown] = useState({ key: '', limit: RESULTS_PAGE });
  const viewKey = `${scanId}:${filter}`;
  const limit = shown.key === viewKey ? shown.limit : RESULTS_PAGE;
  const items = useResource(
    async () => {
      const first = await client.scanItems(scanId, filter, 0, Math.min(limit, 2000));
      let rows = first.rows;
      while (rows.length < Math.min(limit, first.total)) {
        const next = await client.scanItems(scanId, filter, rows.length, Math.min(2000, limit - rows.length));
        if (next.rows.length === 0) break;
        rows = rows.concat(next.rows);
      }
      return { ...first, rows };
    },
    [scanId, filter, limit],
    {
      // A removed track leaves the results in either case.
      events: live ? ['scan.item', 'scan.finished', 'library.changed'] : ['library.changed'],
      throttleMs: 900,
    },
  );
  // Results that did not change keep their objects (and skip rendering).
  const kept = useRef<ScanItemRow[]>([]);
  const rows = useMemo(() => {
    const next = reuseRows(kept.current, items.data?.rows ?? []);
    kept.current = next;
    return next;
  }, [items.data]);
  return (
    <Section
      title="Results"
      aside={
        <TabList
          label="Filter results"
          idBase={idBase}
          panelId={`${idBase}-panel`}
          value={filter}
          items={FILTERS.map((f) => ({ id: f.id, label: f.label }))}
          onChange={setFilter}
        />
      }
    >
      <div role="tabpanel" id={`${idBase}-panel`} aria-labelledby={tabId(idBase, filter)}>
      {items.error ? (
        <ErrorState title="Could not load the results" error={items.error} onRetry={items.reload} />
      ) : rows.length === 0 ? (
        <p className="faint results-empty">{items.loading ? 'Loading…' : filter !== 'all' ? 'No results in this category.' : live ? 'Results appear here as files finish.' : 'No files were analyzed in this scan.'}</p>
      ) : (
        <div className="results-wrap">
          <table className="list-table results-table">
            <thead>
              <tr>
                <th>Title</th>
                <th>Format</th>
                <th className="is-end">Overall</th>
                <th className="is-end">DJ</th>
                <th className="is-end">Listening</th>
                <th>Issues</th>
                <th className="is-end">Finished</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((t) => (
                <ResultRow key={t.id} t={t} />
              ))}
            </tbody>
          </table>
          {items.data && items.data.total > rows.length ? (
            live ? (
              <p className="faint results-more">
                Showing the latest {formatCount(rows.length)} of {formatCount(items.data.total)}. The full list is in{' '}
                <a className="link" href={formatRoute({ view: 'history', scanId })}>
                  Analysis history
                </a>
                .
              </p>
            ) : (
              <p className="faint results-more">
                Showing the latest {formatCount(rows.length)} of {formatCount(items.data.total)}.{' '}
                <button type="button" className="link" disabled={items.loading} onClick={() => setShown({ key: viewKey, limit: limit + RESULTS_PAGE })}>
                  Show {formatCount(Math.min(RESULTS_PAGE, items.data.total - rows.length))} more
                </button>
              </p>
            )
          ) : null}
        </div>
      )}
      </div>
    </Section>
  );
}

export function QueueView() {
  const { client, overview, refreshScan } = useCore();
  const { scan, lastFinished } = useScan();
  const imports = useImports();
  const toast = useToast();
  const history = useResource(() => client.history(1), [], { events: ['scan.finished', 'scan.started'] });
  const busy = scan != null && scan.state !== 'idle';
  const latest = history.data?.[0] ?? null;
  const scanId = (busy ? scan?.id : null) ?? lastFinished?.id ?? latest?.id ?? null;
  // Only files still on disk can be analyzed.
  const pending = overview?.tracks.pendingPresent ?? 0;
  const failed = overview?.tracks.failedPresent ?? 0;

  const act = (fn: () => Promise<unknown>, failMsg: string) =>
    fn().catch((e: unknown) => toast({ tone: 'error', title: failMsg, body: e instanceof Error ? e.message : String(e) }));

  /** Pause, resume and cancel answer `ok: false` when the scan was already in another state: say so and show the real state. */
  const control = (fn: () => Promise<{ ok: boolean }>, failMsg: string, staleMsg: string) =>
    act(
      () =>
        fn().then((r) => {
          if (r.ok) return;
          toast({ tone: 'info', title: staleMsg });
          refreshScan();
        }),
      failMsg,
    );

  const queued = (r: QueueResult) => toast(describeQueued(r));

  const finishedRecord: ScanRecord | ScanStatus | null = !busy ? (lastFinished && lastFinished.id === scanId ? lastFinished : latest) : null;
  const finishedSummary = finishedRecord ? ('summary' in finishedRecord ? finishedRecord.summary : null) : null;

  return (
    <div className="view view-scroll">
      <ViewHeader
        title="Import queue"
        subtitle={
          busy
            ? 'Analysis runs in the background. You can keep using SONORA, or pause and resume at any time.'
            : 'New and changed files are analyzed here. Scans can be paused, resumed or cancelled.'
        }
        actions={
          <>
            {scan?.state === 'running' ? (
              <Button icon={LuPause} onClick={() => void control(client.pause, 'Could not pause', 'The scan is no longer running')}>
                Pause
              </Button>
            ) : null}
            {scan?.state === 'paused' ? (
              <Button variant="primary" icon={LuPlay} onClick={() => void control(client.resume, 'Could not resume', 'The scan is no longer paused')}>
                Resume
              </Button>
            ) : null}
            {busy && scan?.state !== 'cancelling' ? (
              <Button variant="danger" icon={LuSquare} onClick={() => void control(client.cancel, 'Could not cancel', 'The scan had already finished')}>
                Cancel scan
              </Button>
            ) : null}
            {!busy && failed > 0 ? (
              <Button icon={LuRotateCw} onClick={() => void act(() => client.retryFailed().then(queued), 'Could not retry')}>
                Retry {formatCount(failed)} failed
              </Button>
            ) : null}
            {!busy && pending > 0 ? (
              <Button variant="primary" icon={LuPlay} onClick={() => void act(() => client.analyzePending().then(queued), 'Could not start')}>
                Analyze {formatCount(pending)} waiting
              </Button>
            ) : null}
          </>
        }
      />
      {busy && scan ? (
        <Section>
          <LiveScan scan={scan} />
        </Section>
      ) : finishedRecord ? (
        <Section title={finishedRecord.startedAt ? `Last scan, ${formatDate(finishedRecord.startedAt)}` : 'Last scan'}>
          <Summary summary={finishedSummary} record={finishedRecord} />
        </Section>
      ) : null}
      {scanId != null ? (
        <ScanResults key={scanId} scanId={scanId} live={busy} />
      ) : (
        <EmptyState
          icon={LuInbox}
          title="Nothing in the queue"
          actions={
            <>
              <Button variant="primary" icon={LuFolderPlus} onClick={imports.addFolders}>
                Add folder…
              </Button>
              <Button icon={LuPlus} onClick={imports.addFiles}>
                Add files…
              </Button>
            </>
          }
        >
          Add music and the files appear here while they are analyzed, with results as each one finishes.
        </EmptyState>
      )}
    </div>
  );
}
