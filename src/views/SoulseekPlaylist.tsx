/**
 * A YouTube playlist downloaded from Soulseek: paste the link, check the
 * tracks SONORA read from it, start, and follow each track from search to
 * download to the check it gets on arrival. Everything runs in the core
 * (jobs.rs); this view shows a run and sends it instructions.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuChevronDown, LuChevronRight, LuCircleAlert, LuCircleCheck, LuExternalLink, LuListMusic, LuLoaderCircle, LuPause, LuPlay, LuRefreshCw, LuSearch, LuTrash2, LuX } from 'react-icons/lu';
import type { PlaylistItem, PlaylistRun, PlaylistRunState } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { Checkbox, ProgressBar } from '../components/Controls.tsx';
import { useConfirm } from '../components/Dialog.tsx';
import { EmptyState } from '../components/Layout.tsx';
import { QuickRules } from '../components/PlaylistRules.tsx';
import { DASH, formatBytes, formatCount, formatDuration, formatRelative, plural } from '../lib/format.ts';
import { FILTERS, canInclude, itemWords, matches, pickOf, quality, runWords, trackName, type Filter } from '../lib/playlist.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import type { SoulseekState } from './Soulseek.tsx';

/** The run on screen, kept while the person looks at other pages. */
let remembered: number | null = null;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function Candidates({ item, run, onChoose }: { item: PlaylistItem; run: PlaylistRun; onChoose: (c: number) => void }) {
  const pick = item.chosen ?? 0;
  const canChoose = ['found', 'downloading', 'done', 'failed', 'not_found', 'verifying'].includes(item.status) && ['running', 'paused', 'review', 'waiting', 'finished', 'cancelled'].includes(run.state);
  if (item.candidates.length === 0) {
    return <p className="faint pl-detail-empty">{item.status === 'pending' || item.status === 'searching' ? 'Nothing found yet.' : item.responses ? `${plural(item.responses, 'person', 'people')} answered, but with nothing that matched.` : 'No one answered with this track.'}</p>;
  }
  return (
    <table className="mini-table pl-cands">
      <thead>
        <tr>
          <th>File</th>
          <th>From</th>
          <th>Format</th>
          <th className="is-end">Length</th>
          <th className="is-end">Size</th>
          <th>Outcome</th>
        </tr>
      </thead>
      <tbody>
        {item.candidates.map((c, n) => (
          <tr key={`${c.username}\n${c.filename}`} className={c.rejected ? 'is-rejected' : n === pick ? 'is-pick' : undefined}>
            <td className="ellipsis" title={c.filename}>
              {c.name}
            </td>
            <td>
              <span className="sk-user">{c.username}</span> <span className={c.freeSlot ? 't-lime' : 'faint'}>{c.freeSlot ? 'free slot' : `queue ${formatCount(c.queue)}`}</span>
            </td>
            <td>
              {c.ext.toUpperCase() || DASH} <span className="faint">{quality(c)}</span>
            </td>
            <td className="is-end num">{formatDuration(c.length)}</td>
            <td className="is-end num">{formatBytes(c.size)}</td>
            <td>
              {c.rejected ? (
                <span className="faint">{c.rejected}</span>
              ) : n === pick ? (
                <span className="t-lime">{item.status === 'done' ? 'Downloaded' : 'The pick'}</span>
              ) : canChoose ? (
                <Button size="sm" variant="ghost" onClick={() => onChoose(n)}>
                  Use this file
                </Button>
              ) : (
                <span className="faint">Passed the rules</span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Row({
  item,
  run,
  open,
  onToggle,
  onInclude,
  onChoose,
  onOpenUrl,
}: {
  item: PlaylistItem;
  run: PlaylistRun;
  open: boolean;
  onToggle: () => void;
  onInclude: (on: boolean) => void;
  onChoose: (c: number) => void;
  onOpenUrl: (url: string) => void;
}) {
  const draft = run.state === 'draft';
  const words = itemWords(item, run.state === 'review');
  const pick = item.status === 'pending' || item.status === 'searching' ? null : pickOf(item);
  const editable = canInclude(item, run.state);
  return (
    <>
      <tr className={item.included ? undefined : 'is-left-out'} data-item={item.id}>
        <td className="pl-check">
          <Checkbox checked={item.included} disabled={!editable} aria-label={`Look for ${trackName(item)}`} onChange={(e) => onInclude(e.target.checked)} />
        </td>
        <td className="num faint is-end">{item.seq + 1}</td>
        <td className="pl-track">
          <span className="ellipsis selectable" title={item.ytTitle}>
            {trackName(item)}
          </span>
          <span className="faint pl-sub ellipsis">
            {item.mix ? <span>{item.mix}</span> : null}
            {item.catalogue ? <span className="num">{item.catalogue}</span> : null}
            {item.channel ? <span>{item.channel}</span> : null}
            {item.seconds ? <span className="num">{formatDuration(item.seconds)}</span> : null}
          </span>
        </td>
        {draft ? null : (
          <>
            <td className={`pl-status ${words.tone}`}>
              {item.status === 'searching' || item.status === 'verifying' ? <LuLoaderCircle className="spin" aria-hidden /> : null}
              <span>{words.text}</span>
            </td>
            <td className="pl-pick">
              {pick ? (
                <>
                  <span className="ellipsis" title={pick.filename}>
                    {pick.name}
                  </span>
                  <span className="faint pl-sub">
                    <span>{pick.ext.toUpperCase()}</span>
                    <span>{quality(pick)}</span>
                    <span className="sk-user">{pick.username}</span>
                  </span>
                </>
              ) : (
                <span className="faint">{DASH}</span>
              )}
            </td>
          </>
        )}
        <td className="cell-actions pl-act">
          <IconButton size="sm" icon={LuExternalLink} label={`Open ${item.ytTitle} on YouTube`} onClick={() => onOpenUrl(item.url)} />
          {draft ? null : <IconButton size="sm" icon={open ? LuChevronDown : LuChevronRight} label={open ? 'Hide the details' : `Details of ${trackName(item)}`} aria-expanded={open} onClick={onToggle} />}
        </td>
      </tr>
      {open && !draft ? (
        <tr className="pl-detail-row">
          <td colSpan={6}>
            <div className="pl-detail">
              {!item.included && item.leftOut ? <p className="faint">{item.leftOut}</p> : null}
              {item.note ? <p className="pl-note">{item.note}</p> : null}
              {item.error && item.status === 'failed' ? (
                <p className="t-red">
                  <LuCircleAlert aria-hidden /> {item.error}
                </p>
              ) : null}
              {item.track ? (
                <p>
                  In the library as <a className="link" href={formatRoute({ view: 'track', id: item.track.id })}>{item.track.displayTitle}</a>
                  {item.track.verdict ? <span className="faint"> — {item.track.verdict}</span> : null}
                </p>
              ) : null}
              <p className="faint pl-detail-meta">
                {item.variation ? `Found with search ${item.variation} of 5. ` : ''}
                {item.responses != null ? `${plural(item.responses, 'person', 'people')} answered. ` : ''}
                {item.attempts ? `${plural(item.attempts, 'download try', 'download tries')}.` : ''}
              </p>
              <Candidates item={item} run={run} onChoose={onChoose} />
            </div>
          </td>
        </tr>
      ) : null}
    </>
  );
}

function EarlierRuns({ onOpen }: { onOpen: (id: number) => void }) {
  const { client } = useCore();
  const [runs, setRuns] = useState<PlaylistRun[] | null>(null);
  useEffect(() => {
    client.playlistList().then(
      (r) => setRuns(r.runs),
      () => setRuns([]),
    );
  }, [client]);
  if (!runs || runs.length === 0) return null;
  return (
    <section className="pl-earlier" aria-label="Earlier playlists">
      <h3 className="sk-section-title">Earlier playlists</h3>
      <ul className="pl-earlier-list">
        {runs.map((r) => (
          <li key={r.id}>
            <button type="button" className="link" onClick={() => onOpen(r.id)}>
              {r.title ?? r.source}
            </button>
            <span className="faint">
              {r.state === 'draft' ? 'not started' : r.state === 'finished' ? `${formatCount(r.counts.done)} of ${formatCount(r.counts.included)} downloaded` : r.state}, {formatRelative(r.updatedAt)}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function SoulseekPlaylist({ state }: { state: SoulseekState }) {
  const { client, bridge } = useCore();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const [runId, setRunId] = useState<number | null>(remembered);
  const [run, setRun] = useState<(PlaylistRun & { items: PlaylistItem[] }) | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [source, setSource] = useState('');
  const [fetching, setFetching] = useState<{ id: number; videos: number } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  // The left-out list stays open once opened, also as videos move in and out of it.
  const [leftOpen, setLeftOpen] = useState(false);
  const seq = useRef(0);

  useEffect(() => {
    remembered = runId;
  }, [runId]);

  const load = useCallback(
    (id: number) => {
      const n = ++seq.current;
      client.playlistGet(id).then(
        (r) => {
          if (n !== seq.current) return;
          setRun(r);
          setLoadError(null);
          if (r.state !== 'fetching') setFetching(null);
        },
        (e: unknown) => {
          if (n !== seq.current) return;
          setLoadError(message(e));
        },
      );
    },
    [client],
  );

  useEffect(() => {
    if (runId == null) {
      setRun(null);
      return;
    }
    load(runId);
  }, [runId, load]);

  useCoreEvent(['playlist.changed', 'playlist.item', 'playlist.finished'], (name, payload) => {
    if (name === 'playlist.item') {
      const item = payload as PlaylistItem;
      setRun((r) => (r && r.id === item.runId ? { ...r, items: r.items.map((i) => (i.id === item.id ? item : i)) } : r));
      return;
    }
    const p = payload as PlaylistRun | { id: number; state: 'fetching'; videos: number };
    if (p.id !== runId) return;
    if (p.state === 'fetching' && 'videos' in p) {
      setFetching({ id: p.id, videos: p.videos });
      return;
    }
    if (name === 'playlist.finished' || (p as PlaylistRun).state === 'draft' || (p as PlaylistRun).state === 'failed') {
      load(p.id);
      return;
    }
    const summary = p as PlaylistRun;
    setRun((r) => (r && r.id === summary.id ? { ...r, ...summary, items: r.items } : r));
  });

  const openUrl = (url: string) => void bridge.openExternal(url).catch((e: unknown) => toast({ tone: 'error', title: 'Could not open the link', body: message(e) }));

  const act = async (what: string, f: () => Promise<unknown>) => {
    setBusy(what);
    try {
      await f();
    } catch (e) {
      toast({ tone: 'error', title: 'That did not work', body: message(e) });
    } finally {
      setBusy(null);
    }
  };

  const fetch = async () => {
    const text = source.trim();
    if (!text) return;
    setBusy('fetch');
    try {
      const { id } = await client.playlistFetch(text);
      setFetching({ id, videos: 0 });
      setRun(null);
      setRunId(id);
      setFilter('all');
      setOpen(new Set());
    } catch (e) {
      toast({ tone: 'error', title: 'That link cannot be used', body: message(e) });
    } finally {
      setBusy(null);
    }
  };

  // Shown at once; put back if the core says no.
  const include = (ids: number[], on: boolean) => {
    const flip = (cur: (PlaylistRun & { items: PlaylistItem[] }) | null, value: boolean) => (cur ? { ...cur, items: cur.items.map((i) => (ids.includes(i.id) ? { ...i, included: value } : i)) } : cur);
    setRun((cur) => flip(cur, on));
    return act('include', () =>
      client.playlistInclude(runId!, ids, on).then(
        (r) => setRun((cur) => (cur ? { ...cur, ...r, items: cur.items } : cur)),
        (e: unknown) => {
          setRun((cur) => flip(cur, !on));
          throw e;
        },
      ),
    );
  };
  const choose = (itemId: number, c: number) => act('choose', () => client.playlistChoose(runId!, itemId, c).then(() => load(runId!)));
  const discard = async () => {
    if (!run) return;
    const { confirmed } = await confirm({
      title: 'Remove this playlist from SONORA?',
      body: <p>What SONORA noted about it goes: the tracks it read and what was found for each. Downloaded tracks stay in your library.</p>,
      confirmLabel: 'Remove',
      tone: 'danger',
    });
    if (!confirmed) return;
    await act('delete', async () => {
      await client.playlistDelete(run.id);
      setRunId(null);
    });
  };

  const items = run?.items ?? [];
  const shown = useMemo(() => items.filter((i) => matches(i, filter)), [items, filter]);
  const counts = run?.counts;
  const toggle = (id: number) =>
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // ------------------------------------------------------------ entry
  if (runId == null || (loadError && !run)) {
    return (
      <div className="pl-view">
        {loadError ? (
          <p className="sk-error t-amber" role="alert">
            {loadError}
          </p>
        ) : null}
        <form
          className="sk-search-bar"
          onSubmit={(e) => {
            e.preventDefault();
            void fetch();
          }}
        >
          <div className="search sk-search-field">
            <LuListMusic className="search-icon" aria-hidden />
            <input
              className="search-input"
              type="url"
              value={source}
              onChange={(e) => setSource(e.target.value)}
              placeholder="https://www.youtube.com/playlist?list=…"
              aria-label="YouTube playlist link"
              spellCheck={false}
              autoComplete="off"
              maxLength={500}
            />
          </div>
          <Button type="submit" variant="primary" busy={busy === 'fetch'} disabled={!source.trim()}>
            Read the playlist
          </Button>
        </form>
        <EmptyState icon={LuListMusic} title="Download a YouTube playlist from Soulseek">
          <p>
            Paste the link of a public or unlisted playlist. SONORA reads its track list, looks for every track on Soulseek, one search at a time, picks the best file by your{' '}
            <a className="link" href={formatRoute({ view: 'settings' })}>
              download rules
            </a>{' '}
            and downloads it. Each file joins your library and gets checked; a lossy file posing as lossless is swapped for the next best one.
          </p>
          <p className="faint">
            YouTube sees the playlist id, nothing about you or your library. Private playlists (Liked videos, Watch Later) cannot be read: set the playlist to public or unlisted first.
          </p>
        </EmptyState>
        <EarlierRuns onOpen={(id) => setRunId(id)} />
        {dialogs}
      </div>
    );
  }

  // --------------------------------------------------------- fetching
  if (!run || run.state === 'fetching') {
    return (
      <div className="pl-view">
        <p className="sk-summary" aria-live="polite">
          <LuLoaderCircle className="spin" aria-hidden /> Reading the playlist{fetching && fetching.videos > 0 ? `: ${plural(fetching.videos, 'video')} so far` : '…'}
        </p>
      </div>
    );
  }

  if (run.state === 'failed' && items.length === 0) {
    return (
      <div className="pl-view">
        <p className="sk-error t-amber" role="alert">
          {run.error ?? 'The playlist could not be read.'}
        </p>
        <div className="sk-key-actions">
          <Button size="sm" onClick={() => setRunId(null)}>
            Try another link
          </Button>
          <Button size="sm" variant="ghost" busy={busy === 'delete'} onClick={() => void act('delete', () => client.playlistDelete(run.id).then(() => setRunId(null)))}>
            Forget it
          </Button>
        </div>
      </div>
    );
  }

  const playlistUrl = run.playlistId ? `https://www.youtube.com/playlist?list=${run.playlistId}` : null;
  const slskdReady = state.slskd.data?.keySaved ?? false;

  // ------------------------------------------------------------ draft
  if (run.state === 'draft') {
    const included = items.filter((i) => i.included);
    const left = items.filter((i) => !i.included);
    return (
      <div className="pl-view">
        <header className="pl-head">
          <div className="pl-titles">
            <h2 className="pl-title">
              {run.title ?? 'YouTube playlist'}{' '}
              {playlistUrl ? <IconButton size="sm" icon={LuExternalLink} label="Open the playlist on YouTube" onClick={() => openUrl(playlistUrl)} /> : null}
            </h2>
            <p className="faint">
              {plural(included.length, 'track')} to look for{left.length ? `, ${formatCount(left.length)} left out` : ''}. {run.error ? <span className="t-amber">{run.error}</span> : null}
            </p>
          </div>
          <div className="pl-head-actions">
            <Button variant="ghost" onClick={() => setRunId(null)}>
              Another playlist
            </Button>
            <Button variant="ghost" icon={LuTrash2} busy={busy === 'delete'} onClick={() => void discard()}>
              Discard
            </Button>
            <Button variant="primary" icon={LuSearch} busy={busy === 'start'} disabled={included.length === 0 || !slskdReady} data-tip={slskdReady ? undefined : 'Set slskd up first (Setup tab)'} onClick={() => void act('start', () => client.playlistStart(run.id).then(() => load(run.id)))}>
              Start
            </Button>
          </div>
        </header>
        <QuickRules />
        <table className="list-table pl-table is-draft">
          <colgroup>
            <col className="c-check" />
            <col className="c-n" />
            <col className="c-track" />
            <col className="c-act" />
          </colgroup>
          <thead>
            <tr>
              <th>
                <span className="visually-hidden">Look for</span>
              </th>
              <th className="is-end">#</th>
              <th>Track</th>
              <th>
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {included.map((i) => (
              <Row key={i.id} item={i} run={run} open={false} onToggle={() => undefined} onInclude={(on) => void include([i.id], on)} onChoose={() => undefined} onOpenUrl={openUrl} />
            ))}
          </tbody>
        </table>
        {left.length ? (
          <details className="pl-left-out" open={leftOpen} onToggle={(e) => setLeftOpen(e.currentTarget.open)}>
            <summary>
              Left out: {plural(left.length, 'video')} <span className="faint">(Shorts, mixes over 30 minutes, short clips without an artist, repeats, unavailable videos)</span>
            </summary>
            <table className="list-table pl-table is-draft">
              <colgroup>
                <col className="c-check" />
                <col className="c-n" />
                <col className="c-track" />
                <col className="c-act" />
              </colgroup>
              <tbody>
                {left.map((i) => (
                  <Row key={i.id} item={i} run={run} open={false} onToggle={() => undefined} onInclude={(on) => void include([i.id], on)} onChoose={() => undefined} onOpenUrl={openUrl} />
                ))}
              </tbody>
            </table>
            <p className="faint pl-left-hint">Tick a video to look for it after all.</p>
          </details>
        ) : null}
        {dialogs}
      </div>
    );
  }

  // -------------------------------------------------------------- run
  const c = counts ?? run.counts;
  const settled = c.done + c.failed + c.notFound + (run.state === 'review' ? c.found : 0);
  const fraction = c.included > 0 ? settled / c.included : 0;
  const over = ['finished', 'cancelled', 'failed', 'waiting'].includes(run.state);
  const stateIcon = (s: PlaylistRunState): ReactNode => {
    switch (s) {
      case 'finished':
        return <LuCircleCheck className="t-lime" aria-hidden />;
      case 'failed':
      case 'cancelled':
        return <LuCircleAlert className="t-amber" aria-hidden />;
      case 'paused':
        return <LuPause aria-hidden />;
      default:
        return <LuLoaderCircle className="spin" aria-hidden />;
    }
  };
  return (
    <div className="pl-view">
      <header className="pl-head">
        <div className="pl-titles">
          <h2 className="pl-title">
            {run.title ?? 'YouTube playlist'} {playlistUrl ? <IconButton size="sm" icon={LuExternalLink} label="Open the playlist on YouTube" onClick={() => openUrl(playlistUrl)} /> : null}
          </h2>
          <p className="pl-state" aria-live="polite">
            {stateIcon(run.state)} <span>{runWords(run)}</span>
          </p>
          {run.error && run.state === 'running' ? (
            <p className="t-amber pl-run-error" role="alert">
              {run.error}
            </p>
          ) : null}
        </div>
        <div className="pl-head-actions">
          {run.state === 'running' ? (
            <Button icon={LuPause} busy={busy === 'pause'} onClick={() => void act('pause', () => client.playlistPause(run.id))}>
              Pause
            </Button>
          ) : null}
          {run.state === 'paused' || run.state === 'waiting' ? (
            <Button icon={LuPlay} busy={busy === 'resume'} onClick={() => void act('resume', () => client.playlistResume(run.id))}>
              {run.state === 'waiting' ? 'Look again now' : 'Resume'}
            </Button>
          ) : null}
          {run.state === 'review' ? (
            <Button variant="primary" icon={LuPlay} busy={busy === 'confirm'} onClick={() => void act('confirm', () => client.playlistConfirm(run.id))}>
              Download {plural(c.found, 'pick')}
            </Button>
          ) : null}
          {['running', 'paused', 'review', 'waiting'].includes(run.state) ? (
            <Button variant="ghost" className="is-danger-text" icon={LuX} busy={busy === 'cancel'} onClick={() => void act('cancel', () => client.playlistCancel(run.id))}>
              Stop
            </Button>
          ) : null}
          {over && c.notFound + c.failed > 0 ? (
            <Button icon={LuRefreshCw} busy={busy === 'retry'} onClick={() => void act('retry', () => client.playlistRetry(run.id))}>
              Retry {plural(c.notFound + c.failed, 'track')}
            </Button>
          ) : null}
          {over ? (
            <>
              <Button variant="ghost" onClick={() => setRunId(null)}>
                Another playlist
              </Button>
              <IconButton icon={LuTrash2} label="Remove this playlist from SONORA" onClick={() => void discard()} />
            </>
          ) : null}
        </div>
      </header>
      <div className="pl-progress">
        <ProgressBar value={fraction} label="Playlist progress" tone={run.state === 'failed' ? 'amber' : 'lime'} />
        <p className="pl-counts num">
          <span>
            searched {formatCount(c.included - c.pending - c.searching)}/{formatCount(c.included)}
          </span>
          <span className="t-lime">found {formatCount(c.found + c.downloading + c.verifying + c.done)}</span>
          <span>downloading {formatCount(c.downloading)}</span>
          <span className="t-lime">done {formatCount(c.done)}</span>
          <span className={c.notFound ? 't-amber' : ''}>not found {formatCount(c.notFound)}</span>
          <span className={c.failed ? 't-red' : ''}>failed {formatCount(c.failed)}</span>
        </p>
      </div>
      {/* On review the picks are made already (B16); a paused run takes the rules when it resumes. */}
      {run.state === 'paused' ? <QuickRules note="The run uses these from the next search on, once it resumes." /> : null}
      <div className="chip-tabs pl-filters" role="group" aria-label="Show">
        {FILTERS.map((f) => {
          const n = items.filter((i) => matches(i, f.id)).length;
          if (f.id !== 'all' && n === 0) return null;
          return (
            <button key={f.id} type="button" className={`chip-tab ${filter === f.id ? 'is-on' : ''}`} aria-pressed={filter === f.id} onClick={() => setFilter(f.id)}>
              {f.label}
              <span className="cat-count num">{formatCount(n)}</span>
            </button>
          );
        })}
      </div>
      <table className="list-table pl-table">
        <colgroup>
          <col className="c-check" />
          <col className="c-n" />
          <col className="c-track" />
          <col className="c-status" />
          <col className="c-pick" />
          <col className="c-act" />
        </colgroup>
        <thead>
          <tr>
            <th>
              <span className="visually-hidden">Look for</span>
            </th>
            <th className="is-end">#</th>
            <th>Track</th>
            <th>Status</th>
            <th>File</th>
            <th>
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {shown.map((i) => (
            <Row key={i.id} item={i} run={run} open={open.has(i.id)} onToggle={() => toggle(i.id)} onInclude={(on) => void include([i.id], on)} onChoose={(n) => void choose(i.id, n)} onOpenUrl={openUrl} />
          ))}
        </tbody>
      </table>
      {shown.length === 0 ? <p className="faint pl-none">Nothing here yet.</p> : null}
      <p className="faint sk-unverified">
        Formats and sizes are what other people’s clients report. Every file is checked when it arrives; one that turns out lossy or damaged is swapped for the next best, up to three tries, and stays in your library
        for you to decide on. Downloads also show under{' '}
        <a className="link" href={formatRoute({ view: 'soulseek', tab: 'downloads' })}>
          Downloads
        </a>
        .
      </p>
      {dialogs}
    </div>
  );
}

export function openPlaylistTab(): void {
  navigate({ view: 'soulseek', tab: 'playlist' });
}
