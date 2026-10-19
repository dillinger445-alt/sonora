/**
 * Duplicate groups: identical files, identical audio in different files,
 * the same recording in another encoding, and possible alternate versions.
 * Nothing is ever deleted here; removing only drops the library entry.
 */

import { Fragment, useId, useMemo, useState } from 'react';
import { LuCopy, LuExternalLink, LuFingerprint, LuFolderSearch, LuPause, LuPlay, LuTrash2 } from 'react-icons/lu';
import type { DuplicateGroup, DuplicateKind, DuplicateMember } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { ProgressBar, TabList, tabId } from '../components/Controls.tsx';
import { EmptyState, ErrorState, ViewHeader } from '../components/Layout.tsx';
import { FlagList, ScoreCell } from '../components/Marks.tsx';
import {
  DASH,
  encodingShort,
  formatBytes,
  formatCount,
  formatDb,
  formatDuration,
  formatHz,
  formatKbps,
  formatPercent,
  formatRateKhz,
  formatRelative,
  middleEllipsis,
  plural,
} from '../lib/format.ts';
import { focusMain } from '../lib/focus.ts';
import { formatRoute, navigate } from '../lib/router.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { usePlayer } from '../state/player.tsx';
import { useToast } from '../state/toasts.tsx';
import { useTrackActions } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';

const KINDS: { id: DuplicateKind | 'all'; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'exact', label: 'Identical files' },
  { id: 'same_audio', label: 'Identical audio' },
  { id: 'same_recording', label: 'Same recording' },
  { id: 'alternate', label: 'Alternate versions' },
];

const COMPARE_ROWS: { label: string; value: (t: DuplicateMember) => string; num?: boolean }[] = [
  { label: 'Format', value: (t) => t.format ?? t.ext.toUpperCase() },
  { label: 'Sample rate', value: (t) => (t.sampleRate ? `${formatRateKhz(t.sampleRate)} kHz` : DASH), num: true },
  { label: 'Bit depth', value: (t) => (t.bitDepth ? `${t.bitDepth}-bit` : DASH), num: true },
  { label: 'Bitrate', value: (t) => (t.bitrateKbps ? `${formatKbps(t.bitrateKbps)} kbps` : DASH), num: true },
  { label: 'Duration', value: (t) => formatDuration(t.duration), num: true },
  { label: 'Loudness', value: (t) => (t.integratedLufs != null ? `${formatDb(t.integratedLufs)} LUFS` : DASH), num: true },
  { label: 'True peak', value: (t) => (t.truePeak != null ? `${formatDb(t.truePeak, 2, true)} dBTP` : DASH), num: true },
  { label: 'Cutoff', value: (t) => (t.cutoffHz ? formatHz(t.cutoffHz) : 'None'), num: true },
  { label: 'Confidence', value: (t) => t.confidenceLabel ?? DASH },
  { label: 'File size', value: (t) => formatBytes(t.size), num: true },
];

function GroupBlock({ g, actions }: { g: DuplicateGroup; actions: ReturnType<typeof useTrackActions> }) {
  const [compare, setCompare] = useState(false);
  const player = usePlayer();
  const fingerprint = g.kind === 'same_recording' || g.kind === 'alternate';
  // Identical files or identical audio have no better copy; suggest one only between encodings.
  const keep = fingerprint ? g.suggestedKeep : null;
  return (
    <section className={`dup-group kind-${g.kind}`} aria-label={`${g.label}, ${plural(g.members.length, 'copy', 'copies')}`}>
      <header className="dup-head">
        <div className="dup-titles">
          <h3 className="dup-title">
            {g.label}
            <span className="faint num"> {g.members.length}</span>
          </h3>
          <p className="dup-expl faint">{g.explanation}</p>
        </div>
        {fingerprint ? (
          <span className="dup-sim" data-tip="Share of fingerprint bits that agree, after aligning the tracks (50% means unrelated audio)">
            <span className="faint">Match</span> <span className="num">{formatPercent(0.5 + g.similarity / 2)}</span>
          </span>
        ) : null}
        <Button size="sm" variant="ghost" onClick={() => setCompare(!compare)} aria-expanded={compare}>
          {compare ? 'Hide comparison' : 'Compare'}
        </Button>
      </header>
      {compare ? (
        <div className="dup-compare-wrap">
          <table className="compare-table">
            <thead>
              <tr>
                <th>
                  <span className="visually-hidden">Metric</span>
                </th>
                {g.members.map((m) => (
                  <th key={m.id} className="ellipsis" title={m.path}>
                    {m.displayTitle}
                    {m.id === keep ? <span className="keep-badge">{g.kind === 'alternate' ? 'Highest score' : 'Best copy'}</span> : null}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {(['overall', 'dj', 'listening'] as const).map((k) => (
                <tr key={k}>
                  <th scope="row">{k === 'overall' ? 'Overall' : k === 'dj' ? 'DJ' : 'Listening'}</th>
                  {g.members.map((m) => (
                    <td key={m.id}>
                      <ScoreCell score={m[k]} emphasis />
                    </td>
                  ))}
                </tr>
              ))}
              {COMPARE_ROWS.map((r) => (
                <tr key={r.label}>
                  <th scope="row">{r.label}</th>
                  {g.members.map((m) => (
                    <td key={m.id} className={r.num ? 'num' : ''}>
                      {r.value(m)}
                    </td>
                  ))}
                </tr>
              ))}
              <tr>
                <th scope="row">Issues</th>
                {g.members.map((m) => (
                  <td key={m.id}>
                    <FlagList flags={m.flags} max={3} />
                  </td>
                ))}
              </tr>
            </tbody>
          </table>
        </div>
      ) : null}
      <table className="list-table dup-table">
        <colgroup>
          <col className="c-title" />
          <col className="c-format" />
          <col className="c-time" />
          <col className="c-score" />
          {fingerprint ? <col className="c-aligned" /> : null}
          <col />
          <col className="c-actions" />
        </colgroup>
        <thead>
          <tr>
            <th>Title</th>
            <th>Format</th>
            <th className="is-end">Time</th>
            <th className="is-end">Overall</th>
            {fingerprint ? <th className="is-end">Aligned</th> : null}
            <th>Location</th>
            <th>
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {g.members.map((m) => (
            <tr key={m.id} className={m.id === keep ? 'is-keep' : ''}>
              <td className="cell-title-2">
                <span className="dup-name">
                  <a className="ellipsis row-link" href={formatRoute({ view: 'track', id: m.id })}>
                    {m.displayTitle}
                  </a>
                  {m.id === keep ? (
                    <span
                      className="keep-badge"
                      data-tip={
                        g.kind === 'alternate'
                          ? 'Highest overall score in this group. Alternate versions are different edits; you may want to keep several.'
                          : 'Suggested copy to keep: highest score, then lossless, then resolution and bitrate.'
                      }
                    >
                      {g.kind === 'alternate' ? 'Highest score' : 'Best copy'}
                    </span>
                  ) : null}
                </span>
                <span className="ellipsis faint">{m.artist ?? ''}</span>
              </td>
              <td>
                {m.format ?? m.ext.toUpperCase()} <span className="num faint">{m.format ? encodingShort(m) : ''}</span>
              </td>
              <td className="is-end num">{formatDuration(m.duration)}</td>
              <td className="is-end">
                <ScoreCell score={m.overall} emphasis />
              </td>
              {fingerprint ? (
                <td className="is-end num faint" data-tip={m.match ? `Offset ${m.match.offsetSecs} s, ${m.match.matchedSecs} s of matching audio` : undefined}>
                  {m.match ? `${formatCount(m.match.matchedSecs)} s` : 'Reference'}
                </td>
              ) : null}
              <td className="ellipsis faint selectable dup-path" title={m.path}>
                {m.missing ? <span className="t-amber">Missing: </span> : null}
                {middleEllipsis(m.path, 70)}
              </td>
              <td className="cell-actions">
                {/* Listening to copies one after another is the surest comparison. */}
                <IconButton
                  size="sm"
                  label={player.isPlaying(m.id) ? 'Pause' : `Play ${m.displayTitle}`}
                  icon={player.isPlaying(m.id) ? LuPause : LuPlay}
                  className={player.track?.id === m.id ? 'is-current' : ''}
                  onClick={() => player.toggle(m)}
                  disabled={m.missing}
                />
                <IconButton size="sm" label="Open analysis" icon={LuExternalLink} onClick={() => navigate({ view: 'track', id: m.id })} disabled={m.status !== 'done'} />
                <IconButton size="sm" label={actions.revealText} icon={LuFolderSearch} onClick={() => void actions.reveal(m.path)} disabled={m.missing} />
                <IconButton size="sm" label="Remove from library (the file stays on disk)" icon={LuTrash2} tone="danger" onClick={() => void actions.remove([m.id], `“${m.displayTitle}”`).then((ok) => ok && focusMain())} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

export function DuplicatesView() {
  const { client } = useCore();
  const toast = useToast();
  const actions = useTrackActions();
  const [kind, setKind] = useState<DuplicateKind | 'all'>('all');
  const idBase = useId();
  const [progress, setProgress] = useState<number | null>(null);
  const data = useResource(() => client.duplicates(), [], { events: ['duplicates.updated', 'library.changed', 'scan.finished'], throttleMs: 2000 });

  useCoreEvent(['duplicates.progress', 'duplicates.updated'], (name, payload) => {
    if (name === 'duplicates.progress') setProgress((payload as { progress: number }).progress);
    else setProgress(null);
  });

  const d = data.data;
  const scanning = progress != null || (d?.scanning ?? false);
  const counts = useMemo(() => {
    const c: Record<string, number> = { all: d?.groups.length ?? 0 };
    for (const g of d?.groups ?? []) c[g.kind] = (c[g.kind] ?? 0) + 1;
    return c;
  }, [d]);
  const groups = (d?.groups ?? []).filter((g) => kind === 'all' || g.kind === kind);

  const runScan = () => {
    // Shown before the request: on a small library the comparison can finish
    // (and report back) before the request itself returns.
    setProgress(0);
    const failed = (body: string) => {
      setProgress(null);
      toast({ tone: 'error', title: 'Could not compare fingerprints', body });
    };
    client.scanDuplicates().then(
      (r) => {
        if (!r.accepted) failed('The comparison could not be started. Details are in the log.');
      },
      (e: unknown) => failed(e instanceof Error ? e.message : String(e)),
    );
  };

  return (
    <div className="view view-scroll">
      <ViewHeader
        title="Duplicates"
        subtitle="Grouped by file hash, by decoded audio and by acoustic fingerprint, never by title. Nothing is deleted here: removing a copy only takes it out of the library."
        actions={
          <Button variant="primary" icon={LuFingerprint} onClick={() => void runScan()} busy={scanning}>
            {scanning ? 'Comparing…' : 'Find similar recordings'}
          </Button>
        }
      />
      <div className="dup-status">
        {scanning ? (
          <div className="dup-progress">
            <span>Comparing acoustic fingerprints</span>
            <ProgressBar value={progress ?? 0} label="Fingerprint comparison progress" />
            <span className="num faint">{formatPercent(progress ?? 0)}</span>
          </div>
        ) : (
          <p className="faint">
            Identical files and identical audio are found automatically. Similar recordings (another format or bitrate, or an alternate edit) need a fingerprint
            comparison:{' '}
            {d?.fingerprintScannedAt
              ? `last run ${formatRelative(d.fingerprintScannedAt)}${d.analyzedSinceScan ? `; ${plural(d.analyzedSinceScan, 'track')} analyzed since` : ''}.`
              : 'not run yet.'}
          </p>
        )}
      </div>
      {data.error ? <ErrorState title="Could not load duplicates" error={data.error} onRetry={data.reload} /> : null}
      {d && d.groups.length > 0 ? (
        <>
          <TabList
            label="Duplicate kind"
            idBase={idBase}
            panelId={`${idBase}-panel`}
            value={kind}
            items={KINDS.map((k) => ({ id: k.id, label: k.label, count: counts[k.id] ?? 0 }))}
            onChange={setKind}
          />
          <div className="dup-groups" role="tabpanel" id={`${idBase}-panel`} aria-labelledby={tabId(idBase, kind)}>
            {groups.map((g) => (
              <Fragment key={g.id}>
                <GroupBlock g={g} actions={actions} />
              </Fragment>
            ))}
            {groups.length === 0 ? <p className="faint">No groups of this kind.</p> : null}
          </div>
        </>
      ) : d ? (
        <EmptyState icon={LuCopy} title="No duplicates found">
          {d.fingerprintScannedAt
            ? 'No identical files, identical audio or matching recordings in the analyzed tracks.'
            : 'No identical files or identical audio so far. Run “Find similar recordings” to also compare acoustic fingerprints.'}
        </EmptyState>
      ) : null}
      {actions.dialogs}
    </div>
  );
}
