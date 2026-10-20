/**
 * Searching Soulseek through slskd. Results are grouped by folder, the
 * best-connected people first, lossless files only unless asked otherwise.
 * Formats and sizes are what other people's clients report; SONORA checks
 * each file once it arrives.
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import { LuCheck, LuCircleStop, LuDownload, LuListPlus, LuLoaderCircle, LuPause, LuPlay, LuSearch, LuX } from 'react-icons/lu';
import type { FoundFile, FoundFolder, SearchResults, Transfer } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { previewWords } from '../components/PlayerBar.tsx';
import { Checkbox, Segmented } from '../components/Controls.tsx';
import { EmptyState } from '../components/Layout.tsx';
import { DASH, formatBytes, formatCount, formatDuration, formatKbps, formatRateKhz, plural } from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { previewKey, usePlayer, type PreviewSource } from '../state/player.tsx';
import { useToast } from '../state/toasts.tsx';
import type { SoulseekState } from './Soulseek.tsx';

/** Formats a preview can be played of (decoded as they arrive). */
const PREVIEWABLE = new Set(['flac', 'wav', 'wave', 'aif', 'aiff', 'aifc']);

/** Why a file cannot be previewed, or null when it can. */
function previewBlock(x: FoundFile, local: boolean): string | null {
  if (!local) return 'Previews need slskd on this computer.';
  if (x.size <= 0) return 'Its size is unknown, so it cannot be previewed.';
  if (PREVIEWABLE.has(x.ext)) return null;
  if (x.ext === 'm4a' || x.ext === 'mp4' || x.ext === 'alac') return 'M4A files keep what a player needs at their end, so they cannot be played before they arrive whole.';
  return `${x.ext ? x.ext.toUpperCase() : 'This file'} cannot be previewed. Previews are for FLAC, WAV and AIFF.`;
}

function sourceOf(f: FoundFolder, x: FoundFile): PreviewSource {
  return { username: f.username, filename: x.filename, name: x.name, folder: f.name, size: x.size, length: x.length };
}

/** The Soulseek files to listen to next, one after another. */
function PreviewQueue() {
  const player = usePlayer();
  const q = player.queue;
  if (q.length === 0) return null;
  const listening = player.track?.preview != null;
  return (
    <section className="sk-queue" aria-label="Preview queue">
      <div className="sk-section-head">
        <h3 className="sk-section-title">
          Up next <span className="num faint">{formatCount(q.length)}</span>
        </h3>
        <label className="check-row sk-queue-advance">
          <Checkbox checked={player.autoAdvance} onChange={(e) => player.setAutoAdvance(e.target.checked)} />
          <span>Play the next one when a preview ends</span>
        </label>
        {!listening ? (
          <Button size="sm" variant="primary" icon={LuPlay} onClick={player.skipPreview}>
            Play the queue
          </Button>
        ) : null}
        <Button size="sm" variant="ghost" onClick={player.clearQueue}>
          Clear
        </Button>
      </div>
      <ol className="sk-queue-list">
        {q.map((x, i) => (
          <li key={previewKey(x)}>
            <span className="num faint sk-queue-n">{i + 1}</span>
            <span className="ellipsis" title={x.filename}>
              {x.name}
            </span>
            <span className="faint ellipsis">{x.username}</span>
            <span className="sk-queue-act">
              {i > 0 ? (
                <Button size="sm" variant="ghost" onClick={() => player.playPreviewNext(x)}>
                  Play next
                </Button>
              ) : null}
              <IconButton size="sm" icon={LuX} label={`Take ${x.name} out of the queue`} onClick={() => player.removeFromQueue(x)} />
            </span>
          </li>
        ))}
      </ol>
    </section>
  );
}

/** The search on screen, kept while the person looks at other pages. */
let remembered: { id: string; text: string; lossless: boolean } | null = null;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

const fileKey = (username: string, filename: string) => `${username}\n${filename}`;

/**
 * How likely a download is to start soon, as a five-step ladder: a free
 * upload slot and a fast connection light all five; a long queue one.
 */
function availability(f: FoundFolder): { level: number; label: string } {
  if (f.freeSlot) {
    const level = f.speed >= 1_000_000 ? 5 : f.speed >= 300_000 ? 4 : 3;
    return { level, label: f.speed > 0 ? `Free upload slot, ${formatBytes(f.speed)}/s` : 'Free upload slot' };
  }
  return { level: f.queue <= 10 ? 2 : 1, label: f.queue > 0 ? `${plural(f.queue, 'download')} ahead in ${f.username}’s queue` : `No free upload slot at ${f.username}` };
}

function Availability({ f }: { f: FoundFolder }) {
  const a = availability(f);
  return (
    <span className={`sk-avail ${a.level <= 2 ? 'is-low' : ''}`} role="img" aria-label={a.label} data-tip={a.label}>
      {[1, 2, 3, 4, 5].map((i) => (
        <span key={i} className={i <= a.level ? 'is-lit' : undefined} />
      ))}
    </span>
  );
}

/** What the other person's client says about the audio. */
function reported(f: FoundFile): string {
  if (f.lossless) {
    const parts = [f.bitDepth ? `${f.bitDepth}-bit` : null, f.sampleRate ? `${formatRateKhz(f.sampleRate)} kHz` : null].filter(Boolean);
    return parts.length ? parts.join(', ') : DASH;
  }
  return f.bitRate ? `${formatKbps(f.bitRate)} kbps` : DASH;
}

/** Where a file stands in slskd's downloads, in words. */
function transferWords(t: Transfer): string {
  switch (t.state) {
    case 'done':
      return 'Downloaded';
    case 'downloading':
      return t.size > 0 ? `${Math.floor((t.bytes / t.size) * 100)}%` : 'Downloading';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
    default:
      return 'Queued';
  }
}

/**
 * The folder's name, with the folder it sits in when that adds something
 * ("Kerri Chandler" above "Atmosphere", but not above "Kerri Chandler - Atmosphere EP").
 */
function folderTitle(f: FoundFolder): { parent: string | null; name: string } {
  const parts = f.directory.split(/[\\/]/).filter(Boolean);
  const name = parts[parts.length - 1] ?? '';
  // The top of a share ("@@abcde", "Music", a drive) says little.
  const parent = parts.length >= 3 ? (parts[parts.length - 2] ?? null) : null;
  if (!parent || name.toLowerCase().includes(parent.toLowerCase())) return { parent: null, name };
  return { parent, name };
}

function FolderBlock({
  f,
  transfers,
  onDownload,
  busy,
  local,
}: {
  f: FoundFolder;
  transfers: Map<string, Transfer>;
  onDownload: (f: FoundFolder, files: FoundFile[]) => void;
  busy: boolean;
  /** slskd is on this computer (previews need it). */
  local: boolean;
}) {
  const player = usePlayer();
  const current = player.track?.preview ? previewKey(player.track.preview) : null;
  const queued = new Set(player.queue.map(previewKey));
  const wanted = f.files.filter((x) => {
    const t = transfers.get(fileKey(f.username, x.filename));
    return !x.inLibrary && (!t || t.state === 'failed' || t.state === 'cancelled');
  });
  const owned = f.files.filter((x) => x.inLibrary).length;
  const a = availability(f);
  const title = folderTitle(f);
  return (
    <section className="sk-folder" aria-label={`${f.name || 'Shared files'} from ${f.username}`}>
      <header className="sk-folder-head">
        <Availability f={f} />
        <div className="sk-folder-titles">
          <h3 className="sk-folder-name ellipsis" title={f.directory}>
            {title.parent ? <span className="sk-folder-parent">{title.parent} / </span> : null}
            {title.name || 'Shared files'}
          </h3>
          <p className="sk-folder-meta">
            <span className="sk-user">{f.username}</span>
            <span className={f.freeSlot ? 't-lime' : 't-amber'}>{f.freeSlot ? 'Free slot' : `Queue ${formatCount(f.queue)}`}</span>
            {f.speed > 0 ? <span className="num faint">{formatBytes(f.speed)}/s</span> : null}
            <span className="num faint">{plural(f.files.length, 'file')}</span>
            <span className="num faint">{formatBytes(f.size)}</span>
            {f.hidden > 0 ? <span className="faint">{plural(f.hidden, 'other file')} hidden</span> : null}
          </p>
        </div>
        <Button
          size="sm"
          icon={LuDownload}
          busy={busy}
          disabled={wanted.length === 0}
          data-tip={owned > 0 ? `${plural(owned, 'file')} already in your library ${owned === 1 ? 'is' : 'are'} left out` : a.label}
          onClick={() => onDownload(f, wanted)}
        >
          {wanted.length === 0 ? 'All asked for' : wanted.length === f.files.length ? `Download ${plural(wanted.length, 'file')}` : `Download the other ${formatCount(wanted.length)}`}
        </Button>
      </header>
      <table className="list-table sk-files">
        <colgroup>
          <col className="c-play" />
          <col className="c-name" />
          <col className="c-fmt" />
          <col className="c-quality" />
          <col className="c-len" />
          <col className="c-size" />
          <col className="c-act" />
        </colgroup>
        <thead className="visually-hidden">
          <tr>
            <th>Preview</th>
            <th>File</th>
            <th>Format</th>
            <th>Reported quality</th>
            <th>Length</th>
            <th>Size</th>
            <th>Download</th>
          </tr>
        </thead>
        <tbody>
          {f.files.map((x) => {
            const t = transfers.get(fileKey(f.username, x.filename));
            const src = sourceOf(f, x);
            const key = previewKey(src);
            const blocked = previewBlock(x, local);
            const isCurrent = current === key;
            const playing = isCurrent && (player.status === 'playing' || player.status === 'loading');
            const loading = isCurrent && player.status === 'loading';
            const words = isCurrent
              ? player.status === 'error'
                ? (player.error ?? 'Playback failed')
                : player.status === 'playing'
                  ? 'Playing'
                  : player.status === 'paused' && player.preview?.ready
                    ? 'Paused'
                    : previewWords(player.preview, f.username)
              : null;
            return (
              <tr key={x.filename} className={isCurrent ? 'is-previewing' : undefined}>
                <td className="sk-play">
                  <IconButton
                    size="sm"
                    icon={loading ? LuLoaderCircle : playing ? LuPause : LuPlay}
                    className={`sk-play-btn ${isCurrent ? 'is-current' : ''} ${loading ? 'is-loading' : ''}`}
                    label={blocked ?? (playing ? `Pause the preview of ${x.name}` : `Preview ${x.name}`)}
                    disabled={blocked != null}
                    onClick={() => player.togglePreview(src)}
                  />
                </td>
                <td className="sk-name-cell" title={x.filename}>
                  <span className="ellipsis selectable">{x.name}</span>
                  {words ? (
                    <span className={`sk-pv-state ${isCurrent && player.status === 'error' ? 't-amber' : 'faint'}`} aria-live="polite">
                      {words}
                    </span>
                  ) : null}
                </td>
                <td className={x.lossless ? 'sk-fmt' : 'sk-fmt faint'}>{x.ext.toUpperCase() || DASH}</td>
                <td className="num faint">{reported(x)}</td>
                <td className="is-end num">{formatDuration(x.length)}</td>
                <td className="is-end num">{formatBytes(x.size)}</td>
                <td className="cell-actions sk-file-act">
                  {t && t.state !== 'failed' && t.state !== 'cancelled' ? (
                    <span className={`sk-mark ${t.state === 'done' ? 't-lime' : ''}`}>{transferWords(t)}</span>
                  ) : x.inLibrary ? (
                    <span className="sk-mark faint" data-tip="A file with this name and size is in your library">
                      <LuCheck aria-hidden /> In library
                    </span>
                  ) : (
                    <IconButton size="sm" icon={LuDownload} label={`Download ${x.name}`} disabled={busy} onClick={() => onDownload(f, [x])} />
                  )}
                  {blocked == null && !isCurrent ? (
                    <IconButton
                      size="sm"
                      icon={LuListPlus}
                      label={queued.has(key) ? `${x.name} is in the preview queue` : `Add ${x.name} to the preview queue`}
                      disabled={queued.has(key)}
                      onClick={() => player.enqueuePreview(src)}
                    />
                  ) : null}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}

export function SoulseekSearch({ state }: { state: SoulseekState }) {
  const { client } = useCore();
  const toast = useToast();
  const [text, setText] = useState(remembered?.text ?? '');
  const [search, setSearch] = useState<{ id: string; text: string } | null>(remembered ? { id: remembered.id, text: remembered.text } : null);
  const [lossless, setLossless] = useState(remembered?.lossless ?? true);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  /** Folders whose download request is on its way. */
  const [asking, setAsking] = useState<ReadonlySet<string>>(new Set());
  // slskd takes two download requests at a time: SONORA sends them one by one.
  const queue = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    remembered = search ? { ...search, lossless } : null;
  }, [search, lossless]);

  // slskd keeps searching for about 15 seconds and hands over the results
  // when the search ends; until then it tells how many people answered. A
  // search that slskd never finishes is left after three minutes.
  const [stuck, setStuck] = useState(false);
  useEffect(() => {
    if (!search) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const since = Date.now();
    setStuck(false);
    const tick = async () => {
      try {
        const r = await client.slskdResults(search.id, lossless);
        if (stopped) return;
        setResults(r);
        setError(null);
        if (r.state !== 'searching') return;
        if (Date.now() - since > 180_000) setStuck(true);
        else timer = setTimeout(() => void tick(), 1200);
      } catch (e) {
        if (!stopped) setError(message(e));
      }
    };
    void tick();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, [client, search, lossless]);

  const transfers = useMemo(() => {
    const m = new Map<string, Transfer>();
    for (const t of state.downloads.data?.downloads ?? []) {
      const k = fileKey(t.username, t.filename);
      // The newest attempt counts (the list is ordered newest activity first).
      if (!m.has(k)) m.set(k, t);
    }
    return m;
  }, [state.downloads.data]);

  const start = async () => {
    const words = text.trim();
    if (!words) return;
    setStarting(true);
    setError(null);
    try {
      const s = await client.slskdSearch(words);
      setResults(null);
      setSearch(s);
      // slskd answers again (it was started after the page opened, say).
      if (!state.connection?.ok) void state.test();
    } catch (e) {
      setError(message(e));
    } finally {
      setStarting(false);
    }
  };

  const stop = async () => {
    if (!search) return;
    try {
      await client.slskdStop(search.id);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not stop the search', body: message(e) });
    }
  };

  const download = (f: FoundFolder, files: FoundFile[]) => {
    if (files.length === 0) return;
    const key = `${f.username}\n${f.directory}`;
    setAsking((a) => new Set(a).add(key));
    const ask = async () => {
      try {
        const r = await client.slskdDownload(
          f.username,
          files.map((x) => ({ filename: x.filename, size: x.size })),
        );
        state.downloads.reload();
        toast({
          tone: r.failed > 0 ? 'warning' : 'success',
          title: r.failed > 0 ? `slskd queued ${formatCount(r.queued)} of ${plural(files.length, 'file')}` : `Asked ${f.username} for ${plural(r.queued, 'file')}`,
          body: r.failed > 0 ? 'The others were already queued, or slskd could not ask for them.' : 'They arrive in Downloads, and join your library when they finish.',
          action: { label: 'Show downloads', run: () => navigate({ view: 'soulseek', tab: 'downloads' }) },
        });
      } catch (e) {
        toast({ tone: 'error', title: 'slskd could not start the download', body: message(e) });
      } finally {
        setAsking((a) => {
          const next = new Set(a);
          next.delete(key);
          return next;
        });
      }
    };
    queue.current = queue.current.then(ask, ask);
  };

  const searching = results?.state === 'searching';
  const folders = results?.folders ?? [];

  return (
    <div className="sk-search">
      <form
        className="sk-search-bar"
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          void start();
        }}
      >
        <div className="search sk-search-field">
          <LuSearch className="search-icon" aria-hidden />
          <input
            className="search-input"
            type="search"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Artist, title, label or catalogue number"
            aria-label="Search Soulseek"
            spellCheck={false}
            autoComplete="off"
            maxLength={200}
          />
        </div>
        <Button type="submit" variant="primary" busy={starting} disabled={!text.trim()}>
          Search
        </Button>
        <Segmented
          label="Formats"
          size="sm"
          value={lossless ? 'lossless' : 'all'}
          onChange={(v) => setLossless(v === 'lossless')}
          options={[
            { value: 'lossless', label: 'Lossless', hint: 'FLAC, WAV, AIFF, ALAC, WavPack and APE' },
            { value: 'all', label: 'All formats' },
          ]}
        />
      </form>

      {error ? (
        <p className="sk-error t-amber" role="alert">
          {error}
        </p>
      ) : null}

      {search && results ? (
        <div className="sk-summary" aria-live="polite">
          <span>
            {searching && stuck
              ? `slskd has not finished the search for “${results.text || search.text}”. Try searching again, or look at slskd’s web page.`
              : searching
              ? `Searching for “${results.text || search.text}”: ${plural(results.responses, 'person', 'people')} answered so far. The results appear when the search ends.`
              : results.state === 'failed'
                ? `The search for “${results.text || search.text}” failed in slskd.`
                : `${plural(folders.length + results.moreFolders, 'folder')} for “${results.text || search.text}”, from ${plural(results.responses, 'person', 'people')}.`}
            {results.hiddenFiles > 0 ? ` ${plural(results.hiddenFiles, 'lossy file')} hidden.` : ''}
            {results.moreFolders > 0 ? ` Showing the best ${formatCount(folders.length)}.` : ''}
          </span>
          {searching ? (
            <Button size="sm" variant="ghost" icon={LuCircleStop} onClick={() => void stop()}>
              Stop search
            </Button>
          ) : null}
        </div>
      ) : null}

      {search && results && folders.length > 0 ? (
        <>
          <p className="sk-unverified faint">
            Formats, quality and sizes are what other people’s clients report. SONORA checks every file when it arrives. A preview plays a file while it arrives, from
            someone else’s share: it is not checked, kept or added to your library, and the same rules apply to it as to a download.
          </p>
          <PreviewQueue />
          <div className="sk-results">
            {folders.map((f) => (
              <FolderBlock
                key={`${f.username}\n${f.directory}`}
                f={f}
                transfers={transfers}
                onDownload={download}
                busy={asking.has(`${f.username}\n${f.directory}`)}
                local={state.slskd.data?.local ?? false}
              />
            ))}
          </div>
        </>
      ) : search && results && !searching ? (
        <EmptyState icon={LuSearch} title={lossless ? 'No lossless files found' : 'Nothing found'}>
          <p>
            {lossless && results.hiddenFiles > 0
              ? `${plural(results.hiddenFiles, 'file')} in other formats matched. Choose All formats to see them, or try other words.`
              : 'Try fewer or other words. Soulseek matches every word you type against folder and file names.'}
          </p>
        </EmptyState>
      ) : !search ? (
        <EmptyState icon={LuSearch} title="Search for an artist, a title or a release">
          <p>Results list lossless files first: FLAC, WAV and AIFF. Downloads go to slskd, and each finished file joins your library and gets checked.</p>
        </EmptyState>
      ) : null}
    </div>
  );
}
