import { useState } from 'react';
import { LuChevronRight, LuFolderPlus, LuPlay, LuPlus, LuRefreshCw, LuShieldCheck } from 'react-icons/lu';
import type { Overview, ScoreKind } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Segmented } from '../components/Controls.tsx';
import { Section, ViewHeader } from '../components/Layout.tsx';
import { FlagList, ScoreCell } from '../components/Marks.tsx';
import { encodingShort, formatCount, formatDate, formatPercent, formatRelative, formatSpan, plural } from '../lib/format.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { SCORE_HINT, SCORE_KINDS, SCORE_LABEL, scoreBand } from '../lib/scores.ts';
import { useCore, useScanBusy } from '../state/core.tsx';
import { useImports } from '../state/imports.tsx';
import { useToast } from '../state/toasts.tsx';
import { describeQueued } from '../state/trackActions.tsx';

const BIN_LABELS = ['0', '10', '20', '30', '40', '50', '60', '70', '80', '90'];

function Histogram({ bins, average, kind }: { bins: number[]; average: number | null; kind: ScoreKind }) {
  const max = Math.max(1, ...bins);
  const total = bins.reduce((a, b) => a + b, 0);
  return (
    <figure className="histo" aria-label={`${SCORE_LABEL[kind]} score distribution of ${plural(total, 'analyzed track')}`}>
      <div className="histo-plot">
        {bins.map((n, i) => {
          const band = scoreBand(i * 10 + 5)?.band ?? 'none';
          return (
            <div key={i} className="histo-col" data-tip={`${BIN_LABELS[i]}–${i === 9 ? 100 : i * 10 + 9}: ${plural(n, 'track')}`}>
              <span className="histo-n num">{n > 0 ? formatCount(n) : ''}</span>
              <span className={`histo-bar band-${band}`} style={{ height: `${(n / max) * 100}%` }} />
            </div>
          );
        })}
        {average != null ? (
          <div className="histo-avg" style={{ left: `${average}%` }} aria-hidden>
            <span className="histo-avg-label num">avg {average.toFixed(1)}</span>
          </div>
        ) : null}
      </div>
      <div className="histo-axis num" aria-hidden>
        {BIN_LABELS.map((l) => (
          <span key={l}>{l}</span>
        ))}
        <span>100</span>
      </div>
      <table className="visually-hidden">
        <caption>{SCORE_LABEL[kind]} score distribution</caption>
        <thead>
          <tr>
            <th>Score range</th>
            <th>Tracks</th>
          </tr>
        </thead>
        <tbody>
          {bins.map((n, i) => (
            <tr key={i}>
              <td>
                {i * 10} to {i === 9 ? 100 : i * 10 + 9}
              </td>
              <td>{n}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  );
}

function AttentionList({ o }: { o: Overview }) {
  const rows = [
    { id: 'clipping', label: 'Clipping', n: o.clipping, tone: 'red' },
    { id: 'source_concern', label: 'Possible lossy source', n: o.sourceConcerns, tone: 'amber' },
    { id: 'integrity', label: 'Integrity problems', n: o.integrity, tone: 'red' },
    { id: 'failed', label: 'Failed to analyze', n: o.tracks.failed, tone: 'red' },
    { id: 'missing', label: 'Missing files', n: o.tracks.missing, tone: 'amber' },
  ];
  const any = rows.some((r) => r.n > 0);
  return (
    <ul className="attn-list">
      {rows.map((r) => (
        <li key={r.id}>
          <a className={`attn-row ${r.n === 0 ? 'is-zero' : ''}`} href={formatRoute({ view: 'attention', flag: r.id })}>
            <span className={`attn-dot ${r.n > 0 ? `is-${r.tone}` : ''}`} aria-hidden />
            <span className="attn-label">{r.label}</span>
            <span className="num attn-n">{formatCount(r.n)}</span>
            <LuChevronRight className="attn-chev" aria-hidden />
          </a>
        </li>
      ))}
      {!any ? <li className="attn-none faint">No problems found in analyzed tracks.</li> : null}
    </ul>
  );
}

function Formats({ o }: { o: Overview }) {
  const total = o.formats.reduce((a, f) => a + f.count, 0);
  if (total === 0) return <p className="faint">Formats appear once tracks are analyzed.</p>;
  return (
    <div className="formats">
      <div className="formats-bar" aria-hidden>
        {o.formats.map((f, i) => (
          <span key={f.format} className={`formats-seg seg-${i % 6}`} style={{ flexGrow: f.count }} />
        ))}
      </div>
      <table className="mini-table">
        <thead>
          <tr>
            <th>Format</th>
            <th className="is-end">Tracks</th>
            <th className="is-end">Share</th>
            <th className="is-end" data-tip="Average overall score">
              Avg.
            </th>
          </tr>
        </thead>
        <tbody>
          {o.formats.map((f, i) => (
            <tr key={f.format}>
              <td>
                <span className={`swatch seg-${i % 6}`} aria-hidden />
                {f.format}
              </td>
              <td className="is-end num">{formatCount(f.count)}</td>
              <td className="is-end num muted">{formatPercent(f.count / total)}</td>
              <td className="is-end">
                <ScoreCell score={f.avgOverall} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function EmptyOverview() {
  const imports = useImports();
  const { info } = useCore();
  return (
    <div className="welcome">
      <div className="welcome-main">
        <h2 className="welcome-title">Add music to analyze</h2>
        <p className="welcome-text">
          Choose a folder and SONORA checks every track for clipping, lossy sources, damaged files and more, then scores each one for overall quality, DJ use and
          listening. Analysis runs in the background; you can keep working.
        </p>
        <div className="welcome-actions">
          <Button variant="primary" icon={LuFolderPlus} onClick={imports.addFolders}>
            Add folder…
          </Button>
          <Button icon={LuPlus} onClick={imports.addFiles}>
            Add files…
          </Button>
        </div>
        <p className="welcome-note faint">You can also drop files or folders anywhere on this window.</p>
      </div>
      <dl className="welcome-facts">
        <div>
          <dt>Read natively</dt>
          <dd>WAV, AIFF, FLAC</dd>
        </div>
        <div>
          <dt>Through FFmpeg</dt>
          <dd>
            MP3, AAC/M4A, ALAC, Ogg Vorbis, Opus
            {info && !info.ffmpeg.available ? <span className="t-amber"> (FFmpeg not found; see Settings)</span> : null}
          </dd>
        </div>
        <div>
          <dt>
            <LuShieldCheck aria-hidden className="inline-icon" /> Privacy
          </dt>
          <dd>Everything runs on this computer. Audio and results never leave it.</dd>
        </div>
      </dl>
    </div>
  );
}

export function OverviewView() {
  const { overview: o, settings, client } = useCore();
  const busy = useScanBusy();
  const imports = useImports();
  const toast = useToast();
  const [kind, setKind] = useState<ScoreKind | null>(null);
  const k = kind ?? settings?.defaultScore ?? 'overall';

  if (!o) {
    return (
      <div className="view">
        <ViewHeader title="Overview" />
      </div>
    );
  }

  if (o.tracks.total === 0) {
    return (
      <div className="view view-scroll">
        <ViewHeader title="Overview" />
        <EmptyOverview />
      </div>
    );
  }

  const dist = o.distribution.find((d) => d.kind === k)?.bins ?? [];
  const lastScan = o.recentScans[0];
  const analyzePending = () =>
    client.analyzePending().then(
      (r) => toast(describeQueued(r)),
      (e: unknown) => toast({ tone: 'error', title: 'Could not start the analysis', body: e instanceof Error ? e.message : String(e) }),
    );

  return (
    <div className="view view-scroll">
      <ViewHeader
        title="Overview"
        subtitle={
          <>
            {plural(o.tracks.total, 'track')} in {plural(o.folders, 'folder')}
            {o.totalDuration ? `, ${formatSpan(o.totalDuration)} of audio` : ''}.
            {lastScan ? ` Last scan ${formatRelative(lastScan.finishedAt ?? lastScan.startedAt)}.` : ''}
          </>
        }
        actions={
          <>
            {o.tracks.pending > 0 && !busy ? (
              <Button icon={LuPlay} onClick={() => void analyzePending()}>
                Analyze {formatCount(o.tracks.pending)} waiting
              </Button>
            ) : null}
            <Button icon={LuRefreshCw} onClick={() => imports.rescanFolders()} disabled={o.folders === 0}>
              Rescan folders
            </Button>
          </>
        }
      />
      <div className="overview-grid">
        <div className="overview-main">
          <Section
            title="Quality profile"
            aside={
              <Segmented label="Score" size="sm" value={k} onChange={setKind} options={SCORE_KINDS.map((x) => ({ value: x, label: SCORE_LABEL[x], hint: SCORE_HINT[x] }))} />
            }
          >
            {o.tracks.analyzed > 0 ? (
              <div className="profile">
                <Histogram bins={dist} average={o.averages[k]} kind={k} />
                <dl className="profile-stats">
                  {SCORE_KINDS.map((x) => (
                    <div key={x} className={x === k ? 'is-current' : ''}>
                      <dt>{SCORE_LABEL[x]} average</dt>
                      <dd>
                        <ScoreCell score={o.averages[x]} emphasis />
                      </dd>
                    </div>
                  ))}
                  <div>
                    <dt>Analyzed</dt>
                    <dd className="num">
                      {formatCount(o.tracks.analyzed)}
                      <span className="faint"> / {formatCount(o.tracks.total)}</span>
                    </dd>
                  </div>
                  <div>
                    <dt>Lossy files</dt>
                    <dd className="num">{formatPercent(o.tracks.analyzed ? o.lossy / o.tracks.analyzed : 0)}</dd>
                  </div>
                </dl>
              </div>
            ) : (
              <p className="faint">Scores appear here as tracks finish analyzing.</p>
            )}
          </Section>
          <Section
            title="Recently analyzed"
            aside={
              <a className="link" href={formatRoute({ view: 'library' })}>
                Open library
              </a>
            }
          >
            {o.recentTracks.length ? (
              <table className="list-table">
                <thead>
                  <tr>
                    <th>Title</th>
                    <th>Format</th>
                    <th className="is-end">{SCORE_LABEL[k]}</th>
                    <th>Issues</th>
                    <th className="is-end">Analyzed</th>
                  </tr>
                </thead>
                <tbody>
                  {o.recentTracks.map((t) => (
                    <tr key={t.id} className="is-link" onClick={() => (t.status === 'done' ? navigate({ view: 'track', id: t.id }) : navigate({ view: 'attention', flag: 'failed' }))}>
                      <td className="cell-title-2">
                        <a href={formatRoute({ view: 'track', id: t.id })} className="ellipsis row-link" onClick={(e) => e.stopPropagation()}>
                          {t.displayTitle}
                        </a>
                        <span className="ellipsis faint">{t.artist ?? ''}</span>
                      </td>
                      <td>
                        {t.format ?? t.ext.toUpperCase()} <span className="num faint">{t.format ? encodingShort(t) : ''}</span>
                      </td>
                      <td className="is-end">
                        <ScoreCell score={t[k]} emphasis />
                      </td>
                      <td>{t.status === 'failed' ? <span className="t-red">Failed</span> : <FlagList flags={t.flags} />}</td>
                      <td className="is-end faint">{formatRelative(t.analyzedAt)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p className="faint">Nothing analyzed yet.</p>
            )}
          </Section>
        </div>
        <div className="overview-side">
          <Section
            title="Needs attention"
            aside={
              <a className="link" href={formatRoute({ view: 'attention' })}>
                Show all
              </a>
            }
          >
            <AttentionList o={o} />
          </Section>
          <Section title="Formats">
            <Formats o={o} />
          </Section>
          <Section
            title="Recent scans"
            aside={
              <a className="link" href={formatRoute({ view: 'history' })}>
                History
              </a>
            }
          >
            {o.recentScans.length ? (
              <ul className="scan-list">
                {o.recentScans.map((s) => (
                  <li key={s.id}>
                    <a className="scan-row" href={formatRoute({ view: 'history', scanId: s.id })}>
                      <span className="scan-date">{formatDate(s.startedAt)}</span>
                      <span className="muted scan-counts">
                        <span className="num">{formatCount(s.completed)}</span> analyzed
                        {s.failed ? (
                          <span className="t-red">
                            , <span className="num">{formatCount(s.failed)}</span> failed
                          </span>
                        ) : null}
                      </span>
                      <span className={`scan-state state-${s.state}`}>{stateLabel(s.state)}</span>
                    </a>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="faint">No scans yet.</p>
            )}
          </Section>
        </div>
      </div>
    </div>
  );
}

export function stateLabel(state: string): string {
  switch (state) {
    case 'completed':
      return 'Completed';
    case 'cancelled':
      return 'Cancelled';
    case 'interrupted':
      return 'Interrupted';
    case 'running':
      return 'Running';
    case 'paused':
      return 'Paused';
    case 'indexing':
      return 'Indexing';
    case 'cancelling':
      return 'Stopping';
    default:
      return state;
  }
}
