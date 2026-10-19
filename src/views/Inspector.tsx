/** Right-hand panel with a quick read of the focused track. */

import { LuDisc3, LuExternalLink, LuFolderSearch, LuListMinus, LuListMusic, LuLoaderCircle, LuPause, LuPencil, LuPlay, LuRefreshCw, LuTrash2, LuUsb } from 'react-icons/lu';
import type { KeyNotation, TrackDetail, TrackRow } from '../api/types.ts';
import { AddToPlaylist } from '../components/AddToPlaylist.tsx';
import { Button } from '../components/Button.tsx';
import { BpmText, KeyBadge, SourceNote } from '../components/DjMarks.tsx';
import { EvidenceMark, ScoreMeter, SeverityIcon, StatusMark } from '../components/Marks.tsx';
import { DASH, encodingShort, formatBytes, formatDb, formatDuration, formatHz, formatKbps, middleEllipsis, plural } from '../lib/format.ts';
import { useDebouncedValue } from '../lib/persist.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { SCORE_HINT, SCORE_KINDS, SCORE_LABEL, SEVERITY } from '../lib/scores.ts';
import { useCore } from '../state/core.tsx';
import { usePlayer } from '../state/player.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import type { TrackActions } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';

/** Tempo, key and genre with where each came from. */
export function DjStrip({ row, notation }: { row: TrackRow; notation: KeyNotation }) {
  return (
    <dl className="dj-strip">
      <div>
        <dt>BPM</dt>
        <dd className="dj-strip-value">{row.djBpm != null ? <BpmText bpm={row.djBpm} variable={row.tempoVariable} /> : <span className="faint">{DASH}</span>}</dd>
        <dd>
          <SourceNote source={row.djBpmSource} />
        </dd>
      </div>
      <div>
        <dt>Key</dt>
        <dd className="dj-strip-value">{row.djKey != null ? <KeyBadge keyId={row.djKey} notation={notation} size="lg" /> : <span className="faint">{DASH}</span>}</dd>
        <dd>
          <SourceNote source={row.djKeySource} />
        </dd>
      </div>
      <div className="is-wide">
        <dt>Genre</dt>
        <dd className="dj-strip-value dj-strip-genre selectable" title={row.djGenre ?? undefined}>
          {row.djGenre ?? <span className="faint">{DASH}</span>}
        </dd>
        <dd>
          <SourceNote source={row.djGenreSource} />
          {row.discogsUrl && row.djGenreSource === 'discogs' ? <DiscogsLink url={row.discogsUrl} /> : null}
        </dd>
      </div>
    </dl>
  );
}

/** The Discogs release a genre came from (Discogs asks for this attribution). */
export function DiscogsLink({ url }: { url: string }) {
  const { bridge } = useCore();
  return (
    <button type="button" className="link dj-discogs-link" onClick={() => void bridge.openExternal(url).catch(() => undefined)} data-tip="Data provided by Discogs: open the release page">
      release
    </button>
  );
}

/** The playlists that hold a track, as links. */
function InPlaylists({ id }: { id: number }) {
  const { client } = useCore();
  const pl = usePlaylists();
  const holding = useResource(() => client.playlistsContaining(id), [id], { events: ['playlists.changed'], throttleMs: 400 });
  const names = (holding.data?.ids ?? []).map((x) => pl.byId.get(x)).filter((n) => n != null);
  if (!names.length) return null;
  return (
    <p className="insp-playlists">
      <LuListMusic aria-hidden />
      <span>
        In{' '}
        {names.map((n, i) => (
          <span key={n.id}>
            {i > 0 ? ', ' : ''}
            <a className="link" href={formatRoute({ view: 'playlist', id: n.id })}>
              {n.name}
            </a>
          </span>
        ))}
      </span>
    </p>
  );
}

export function Inspector({
  row,
  selectionCount,
  actions,
  onRemove,
  onClearSelection,
  selectedIds,
  playlist,
}: {
  row: TrackRow | null;
  selectionCount: number;
  selectedIds: number[];
  actions: TrackActions;
  onRemove: (ids: number[], what?: string) => Promise<void>;
  onClearSelection: () => void;
  /** Shown inside a playlist: taking tracks out of it. */
  playlist?: { id: number; name: string; remove: (ids: number[]) => void };
}) {
  const { client, settings } = useCore();
  const player = usePlayer();
  const notation = settings?.keyNotation ?? 'camelot';
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
          {playlist ? (
            <Button icon={LuListMinus} onClick={() => playlist.remove(selectedIds)}>
              Take out of “{playlist.name}”
            </Button>
          ) : null}
          <AddToPlaylist ids={selectedIds} />
          <Button icon={LuPencil} onClick={() => actions.editDj({ ids: selectedIds })}>
            Edit BPM, key and genre…
          </Button>
          <Button icon={LuUsb} onClick={() => actions.exportDj(selectedIds)}>
            Export for DJ software…
          </Button>
          <Button icon={LuDisc3} onClick={() => void actions.lookupGenres(selectedIds)}>
            Look up genres on Discogs…
          </Button>
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

  const current = player.track?.id === row.id;
  const playing = current && (player.status === 'playing' || player.status === 'loading');
  return (
    <aside className="inspector" aria-label="Track details">
      <div className="insp-head insp-head-play">
        {!row.missing ? (
          <button
            type="button"
            className={`insp-play ${current ? 'is-current' : ''}`}
            aria-label={playing ? 'Pause' : `Play ${row.displayTitle}`}
            data-tip={playing ? 'Pause (P)' : 'Play (P)'}
            onClick={() => player.toggle(row)}
          >
            {current && player.status === 'loading' ? <LuLoaderCircle className="spin" aria-hidden /> : playing ? <LuPause aria-hidden /> : <LuPlay aria-hidden />}
          </button>
        ) : null}
        <div className="insp-head-text">
          <h2 className="insp-title selectable">{row.displayTitle}</h2>
          <p className="insp-sub selectable">{[row.artist, row.album].filter(Boolean).join(' – ') || row.fileName}</p>
        </div>
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

      <section className="insp-dj" aria-label="Tempo, key and genre">
        <DjStrip row={row} notation={notation} />
        <div className="insp-dj-actions">
          <AddToPlaylist ids={[row.id]} size="sm" label="Add to playlist" />
          {playlist ? (
            <Button size="sm" icon={LuListMinus} variant="ghost" onClick={() => playlist.remove([row.id])} data-tip={`Take it out of “${playlist.name}”; it stays in the library`}>
              Take out
            </Button>
          ) : null}
          <Button size="sm" icon={LuPencil} onClick={() => actions.editDj({ ids: [row.id], row })}>
            Edit…
          </Button>
          <Button size="sm" icon={LuUsb} onClick={() => actions.exportDj([row.id])} disabled={row.missing}>
            Export…
          </Button>
          <Button size="sm" icon={LuDisc3} variant="ghost" onClick={() => void actions.lookupGenres([row.id])} data-tip="Look up the genre on Discogs (sends artist and title)">
            Discogs
          </Button>
        </div>
        <InPlaylists id={row.id} />
      </section>

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
