/**
 * Downloads: what slskd is fetching and has fetched, what became of each
 * finished file in the library (and how it scored), and the downloads held
 * for review when the person chose to look at each one first.
 */

import { useState } from 'react';
import { LuDownload, LuExternalLink, LuListChecks, LuX } from 'react-icons/lu';
import type { SourceFile, Transfer } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { ProgressBar } from '../components/Controls.tsx';
import { EmptyState, ErrorState } from '../components/Layout.tsx';
import { FlagList, ScoreCell } from '../components/Marks.tsx';
import { formatBytes, formatCount, formatRelative, plural } from '../lib/format.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import { useResource } from '../state/useResource.ts';
import type { SoulseekState } from './Soulseek.tsx';

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function Review({ files, onDecide }: { files: SourceFile[]; onDecide: (paths: string[], add: boolean) => Promise<void> }) {
  const [busy, setBusy] = useState<string | null>(null);
  const run = async (key: string, paths: string[], add: boolean) => {
    setBusy(key);
    try {
      await onDecide(paths, add);
    } finally {
      setBusy(null);
    }
  };
  return (
    <section className="sk-review" aria-label="Waiting for your review">
      <div className="sk-section-head">
        <h3 className="sk-section-title">
          Waiting for you <span className="num faint">{formatCount(files.length)}</span>
        </h3>
        <p className="faint">These downloads finished. Add them to your library, or keep them out; either way the files stay in the download folder.</p>
        <Button size="sm" variant="primary" icon={LuListChecks} busy={busy === '*'} onClick={() => void run('*', files.map((f) => f.path), true)}>
          Add all
        </Button>
      </div>
      <table className="list-table">
        <thead>
          <tr>
            <th>File</th>
            <th className="is-end">Size</th>
            <th>Arrived</th>
            <th>
              <span className="visually-hidden">Choice</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {files.map((f) => (
            <tr key={f.path}>
              <td className="ellipsis selectable" title={f.path}>
                {f.name}
              </td>
              <td className="is-end num">{formatBytes(f.size)}</td>
              <td className="faint">{formatRelative(f.foundAt)}</td>
              <td className="cell-actions sk-review-act">
                <Button size="sm" busy={busy === f.path} onClick={() => void run(f.path, [f.path], true)}>
                  Add to library
                </Button>
                <Button size="sm" variant="ghost" disabled={busy === f.path} onClick={() => void run(`${f.path}-out`, [f.path], false)}>
                  Keep out
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Where one download stands, in words (and with its score once SONORA analysed it). */
function Status({ t, folderSet }: { t: Transfer; folderSet: boolean }) {
  switch (t.state) {
    case 'downloading': {
      const p = t.size > 0 ? t.bytes / t.size : 0;
      return (
        <div className="sk-progress">
          <ProgressBar value={p} label={`Downloading ${t.name}`} size="sm" />
          <span className="num faint">
            {Math.floor(p * 100)}%{t.speed > 0 ? `, ${formatBytes(t.speed)}/s` : ''}
          </span>
        </div>
      );
    }
    case 'queued':
      return <span>{t.position != null ? `Number ${formatCount(t.position)} in their queue` : 'In their queue'}</span>;
    case 'waiting':
      return <span>Waiting in slskd</span>;
    case 'requested':
      return <span>Asked for</span>;
    case 'starting':
      return <span>Starting</span>;
    case 'failed':
      return <span className="t-amber">{t.detail ?? 'The download failed'}</span>;
    case 'cancelled':
      return <span className="faint">Cancelled</span>;
    case 'done': {
      const l = t.local;
      if (!l)
        return (
          <span className="faint">
            {folderSet ? 'Finished. SONORA adds it when it next looks at the download folder.' : 'Finished in slskd. Choose its download folder in Setup so SONORA can add it.'}
          </span>
        );
      if (l.state === 'review') return <span className="t-amber">Finished, waiting for your review</span>;
      if (l.state === 'importing') return <span>Adding to the library</span>;
      if (l.state === 'declined') return <span className="faint">Kept out of the library</span>;
      if (l.state === 'failed') return <span className="t-amber">Could not be added{l.error ? `: ${l.error}` : ''}</span>;
      const track = l.track;
      if (!track) return <span className="faint">Added, then removed from the library</span>;
      if (track.status === 'pending' || track.status === 'analyzing') return <span>Added, being analysed</span>;
      if (track.status === 'failed') return <span className="t-amber">Added, but the analysis failed</span>;
      return (
        <span className="sk-verdict">
          <ScoreCell score={track.overall} emphasis />
          <span className="ellipsis">{track.verdict ?? 'Analysed'}</span>
          <FlagList flags={track.flags} max={1} />
        </span>
      );
    }
  }
}

export function SoulseekDownloads({ state }: { state: SoulseekState }) {
  const { client } = useCore();
  const toast = useToast();
  const review = useResource(() => client.sourcesReview(), [], { events: ['sources.changed', 'sources.imported'] });
  const [clearing, setClearing] = useState(false);
  const keySaved = state.slskd.data?.keySaved ?? false;
  const folderSet = !!state.sources.data?.folder;
  const list = state.downloads.data;

  const decide = async (paths: string[], add: boolean) => {
    try {
      const r = await client.sourcesDecide(paths, add);
      review.reload();
      state.sources.reload();
      state.downloads.reload();
      if (add) toast({ tone: 'success', title: `Adding ${plural(r.added, 'download')} to the library` });
    } catch (e) {
      toast({ tone: 'error', title: add ? 'Could not add the downloads' : 'Could not keep the downloads out', body: message(e) });
    }
  };

  const cancel = async (t: Transfer) => {
    try {
      await client.slskdCancel(t.username, t.id);
      state.downloads.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not cancel the download', body: message(e) });
    }
  };

  const clear = async () => {
    setClearing(true);
    try {
      await client.slskdClearFinished();
      state.downloads.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not clear the list', body: message(e) });
    } finally {
      setClearing(false);
    }
  };

  const held = review.data ?? [];
  const items = list?.downloads ?? [];
  const finished = items.length - (list?.active ?? 0);

  return (
    <div className="sk-downloads">
      {held.length > 0 ? <Review files={held} onDecide={decide} /> : null}

      {!keySaved ? (
        held.length === 0 ? (
          <EmptyState icon={LuDownload} title="Nothing waiting">
            <p>
              Downloads made with slskd are listed here once it is <a className="link" href={formatRoute({ view: 'soulseek', tab: 'setup' })}>set up</a>. Files arriving in
              the download folder from another Soulseek client go straight to your library.
            </p>
          </EmptyState>
        ) : null
      ) : state.connection && !state.connection.ok ? (
        <ErrorState title="slskd did not answer" error={state.connection.error} onRetry={() => void state.test()} />
      ) : state.downloads.error && !list ? (
        <ErrorState title="Could not get the downloads from slskd" error={state.downloads.error} onRetry={state.downloads.reload} />
      ) : list && items.length === 0 ? (
        <EmptyState icon={LuDownload} title="No downloads yet">
          <p>Search, then choose Download on a file or a folder. slskd fetches it, and SONORA adds it to your library when it finishes.</p>
        </EmptyState>
      ) : list ? (
        <section aria-label="Downloads">
          <div className="sk-section-head">
            <h3 className="sk-section-title">
              slskd’s downloads <span className="num faint">{formatCount(items.length)}</span>
            </h3>
            <p className="faint">
              {list.active > 0 ? `${plural(list.active, 'download')} on the way. ` : ''}
              {state.downloads.error ? <span className="t-amber">{state.downloads.error.message}</span> : null}
            </p>
            <Button size="sm" variant="ghost" busy={clearing} disabled={finished === 0} onClick={() => void clear()} data-tip="Takes finished downloads off this list and slskd’s. The files stay where they are.">
              Clear finished
            </Button>
          </div>
          <table className="list-table sk-transfers">
            <colgroup>
              <col className="c-file" />
              <col className="c-from" />
              <col className="c-status" />
              <col className="c-size" />
              <col className="c-act" />
            </colgroup>
            <thead>
              <tr>
                <th>File</th>
                <th>From</th>
                <th>Status</th>
                <th className="is-end">Size</th>
                <th>
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {items.map((t) => {
                const active = !['done', 'failed', 'cancelled'].includes(t.state);
                const track = t.local?.track;
                return (
                  <tr key={t.id}>
                    <td className="cell-title-2">
                      <span className="ellipsis selectable" title={t.filename}>
                        {t.name}
                      </span>
                      <span className="ellipsis faint">{t.folder}</span>
                    </td>
                    <td className="ellipsis faint">{t.username}</td>
                    <td className="sk-status">
                      <Status t={t} folderSet={folderSet} />
                    </td>
                    <td className="is-end num">{formatBytes(t.size)}</td>
                    <td className="cell-actions">
                      {active ? (
                        <IconButton size="sm" icon={LuX} label={`Cancel ${t.name}`} onClick={() => void cancel(t)} />
                      ) : track && track.status === 'done' ? (
                        <a className="icon-btn icon-btn-sm" href={formatRoute({ view: 'track', id: track.id })} aria-label={`Open the analysis of ${t.name}`} data-tip="Open analysis">
                          <LuExternalLink aria-hidden />
                        </a>
                      ) : null}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {list.more > 0 ? <p className="faint sk-gap">{plural(list.more, 'older download')} not listed.</p> : null}
        </section>
      ) : null}
    </div>
  );
}
