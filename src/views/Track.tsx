/** Full analysis of one track. */

import { useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  LuArrowLeft,
  LuChevronDown,
  LuChevronRight,
  LuDownload,
  LuFolderSearch,
  LuLoaderCircle,
  LuPause,
  LuPlay,
  LuRefreshCw,
  LuTrash2,
  LuTriangleAlert,
  LuUsb,
} from 'react-icons/lu';
import type { Evidence, Finding, Measurements, ScanItemEvent, StoredAnalysis, TrackDetail } from '../api/types.ts';
import { Button, IconButton } from '../components/Button.tsx';
import { BpmText, KeyBadge } from '../components/DjMarks.tsx';
import { ErrorState, ViewHeader } from '../components/Layout.tsx';
import { EvidenceMark, ScoreCell, ScoreMeter, SeverityIcon, StatusMark } from '../components/Marks.tsx';
import { Lanes } from '../charts/Lanes.tsx';
import { BandBars, SpectrumChart } from '../charts/Spectrum.tsx';
import {
  DASH,
  formatBytes,
  formatChannels,
  formatCount,
  formatDate,
  formatDb,
  formatDuration,
  formatHz,
  formatKbps,
  formatPercent,
  formatRateKhz,
  formatRelative,
  formatTimecode,
  plural,
} from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import { EVIDENCE, EVIDENCE_ORDER, SCORE_HINT, SCORE_KINDS, SCORE_LABEL, SEVERITY } from '../lib/scores.ts';
import { focusMain } from '../lib/focus.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { usePlayer } from '../state/player.tsx';
import { useTrackActions } from '../state/trackActions.tsx';
import { useResource } from '../state/useResource.ts';
import { kindLabel } from './History.tsx';
import { TrackDj } from './TrackDj.tsx';

type Focus = { time: number; nonce: number } | null;

/** Outcome of this track in a scan (`scan_items.outcome`). */
const OUTCOME_LABEL: Record<string, string> = {
  done: 'Analyzed',
  failed: 'Failed',
  queued: 'Waiting',
  running: 'Analyzing',
  cancelled: 'Cancelled',
  interrupted: 'Interrupted',
  resumed: 'Continued in a later scan',
  skipped: 'Skipped',
};

function Spec({ label, children, tip }: { label: string; children: ReactNode; tip?: string }) {
  return (
    <div className="spec">
      <dt data-tip={tip}>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/** Up to `n` entries spread evenly over a sorted list. */
function spread(times: number[], n: number): number[] {
  if (times.length <= n) return times;
  return Array.from({ length: n }, (_, i) => times[Math.round((i * (times.length - 1)) / (n - 1))] as number);
}

function FindingItem({ f, times, onTime }: { f: Finding; times: number[]; onTime: (t: number) => void }) {
  const [open, setOpen] = useState(false);
  const sev = SEVERITY[f.severity];
  const penalized = f.penalty.overall > 0 || f.penalty.dj > 0 || f.penalty.listening > 0;
  const id = `finding-${f.id}`;
  return (
    <li className={`finding tone-${sev.tone} ${f.severity === 'info' ? 'is-note' : ''} ${open ? 'is-open' : ''}`}>
      <button type="button" className="finding-head" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <SeverityIcon severity={f.severity} />
        <span className="finding-titles">
          <span className="finding-title">{f.title}</span>
          <span className="finding-summary">{f.summary}</span>
        </span>
        <EvidenceMark evidence={f.evidence} />
        {penalized ? (
          <span className="finding-penalty num" data-tip="Points deducted from the overall score (after confidence weighting)">
            −{f.penalty.overall.toFixed(f.penalty.overall < 10 ? 1 : 0)}
          </span>
        ) : (
          <span className="finding-penalty" aria-hidden />
        )}
        {open ? <LuChevronDown className="finding-chev" aria-hidden /> : <LuChevronRight className="finding-chev" aria-hidden />}
      </button>
      {open ? (
        <div className="finding-body" id={id}>
          <dl className="finding-text">
            <div>
              <dt>What was measured</dt>
              <dd className="selectable">{f.detail}</dd>
            </div>
            <div>
              <dt>Why it matters</dt>
              <dd className="selectable">{f.impact}</dd>
            </div>
            <div>
              <dt>How reliable this is</dt>
              <dd className="selectable">
                {EVIDENCE[f.evidence].label}
                {f.evidence !== 'fact' ? (
                  <span className="faint">
                    , confidence <span className="num">{f.confidence.toFixed(2)}</span>
                  </span>
                ) : null}
                . {f.reliability}
              </dd>
            </div>
            {f.recommendation ? (
              <div>
                <dt>What to do</dt>
                <dd className="selectable">{f.recommendation}</dd>
              </div>
            ) : null}
          </dl>
          {f.metrics.length ? (
            <table className="metrics">
              <tbody>
                {f.metrics.map((m) => (
                  <tr key={m.label}>
                    <th scope="row">{m.label}</th>
                    <td className="num selectable">{m.value}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}
          {times.length ? (
            <div className="finding-times">
              <span className="faint">{times.length > 16 ? 'For example at' : 'At'}</span>
              {spread(times, 16).map((t, i) => (
                <button key={`${t}-${i}`} type="button" className="time-chip num" onClick={() => onTime(t)} data-tip="Show this moment in the graphs">
                  {formatTimecode(t)}
                </button>
              ))}
            </div>
          ) : null}
          <div className="finding-impact">
            <span className="faint">Score impact</span>
            {SCORE_KINDS.map((k) => (
              <span key={k} className="impact">
                {SCORE_LABEL[k]} <span className="num">{f.penalty[k] > 0 ? `−${f.penalty[k].toFixed(1)}` : '0'}</span>
              </span>
            ))}
            {f.penalty.nominalOverall > f.penalty.overall + 0.05 ? (
              <span className="faint" data-tip="The nominal deduction, weighted by how confident this finding is. In the total, further findings with the same root cause count at 25 %, so one cause is not counted twice.">
                (nominal −{f.penalty.nominalOverall.toFixed(1)})
              </span>
            ) : null}
          </div>
        </div>
      ) : null}
    </li>
  );
}

/** Most important first: severity, then deduction, then evidence strength. */
function byImportance(a: Finding, b: Finding): number {
  return (
    SEVERITY[b.severity].rank - SEVERITY[a.severity].rank ||
    b.penalty.overall - a.penalty.overall ||
    EVIDENCE_ORDER.indexOf(a.evidence) - EVIDENCE_ORDER.indexOf(b.evidence)
  );
}

function isNote(f: Finding): boolean {
  return f.severity === 'info' && f.penalty.overall === 0 && f.penalty.dj === 0 && f.penalty.listening === 0;
}

function Findings({ findings, onTime, timesFor }: { findings: Finding[]; onTime: (t: number) => void; timesFor: (f: Finding) => number[] }) {
  const issues = findings.filter((f) => !isNote(f)).sort(byImportance);
  const notes = findings.filter(isNote).sort(byImportance);
  if (findings.length === 0) return <p className="faint">No findings.</p>;
  return (
    <div className="findings">
      <p className="evidence-key faint">
        Each finding shows how it was established:{' '}
        {EVIDENCE_ORDER.map((e, i) => (
          <span key={e} className="evidence-key-item" data-tip={EVIDENCE[e].description}>
            <EvidenceMark evidence={e as Evidence} withLabel={false} /> {EVIDENCE[e].label.toLowerCase()}
            {i < EVIDENCE_ORDER.length - 1 ? ',' : '.'}
          </span>
        ))}
      </p>
      {issues.length ? (
        <ul className="finding-list" aria-label="Issues">
          {issues.map((f) => (
            <FindingItem key={f.id} f={f} times={timesFor(f)} onTime={onTime} />
          ))}
        </ul>
      ) : (
        <p className="no-issues">No issues found.</p>
      )}
      {notes.length ? (
        <section className="finding-group" aria-label="Notes">
          <h3 className="finding-group-title">Notes and checks</h3>
          <ul className="finding-list">
            {notes.map((f) => (
              <FindingItem key={f.id} f={f} times={timesFor(f)} onTime={onTime} />
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function Row({ label, value, tip, tone }: { label: string; value: ReactNode; tip?: string; tone?: 'red' | 'amber' }) {
  // Measurements read as numbers; words ("Stereo", "None") stay in the text face.
  const numeric = typeof value !== 'string' || /^[-+−]?[\d.]/.test(value);
  return (
    <div className="tech-row">
      <dt data-tip={tip}>{label}</dt>
      <dd className={`selectable ${numeric ? 'num' : ''} ${tone ? `t-${tone}` : ''}`}>{value}</dd>
    </div>
  );
}

const CHECKSUM: Record<string, string> = {
  verified: 'Verified (MD5 matches)',
  mismatch: 'Mismatch: decoded audio differs',
  not_stored: 'Not stored by the encoder',
  not_checked: 'Not checked (decoding stopped early)',
  not_applicable: 'Not part of this format',
};

/**
 * `steepCutoff`: whether the core counts the spectral edge as a steep cutoff
 * (the track's `cutoffHz`, decided from unrounded measurements), so this
 * view agrees with the library's Cutoff column.
 */
function TechTables({ a, steepCutoff }: { a: StoredAnalysis; steepCutoff: boolean }) {
  const m: Measurements = a.measurements;
  const f = a.facts;
  const clip = m.clipping;
  const st = m.stereo;
  const edge = steepCutoff ? m.bandwidth.edge : null;
  const db = (v: number | null | undefined, unit: string, d = 1, signed = false) => (v == null ? DASH : `${formatDb(v, d, signed)} ${unit}`);
  const perChannel = (vals: number[], unit: string) => (vals.length > 1 ? vals.map((v) => formatDb(v, 1)).join(' / ') + ` ${unit}` : DASH);
  return (
    <div className="tech">
      <section className="tech-block">
        <h3 className="tech-title">File and encoding</h3>
        <dl>
          <Row label="Container" value={f.containerName || DASH} />
          <Row label="Codec" value={`${f.codecName}${f.codecProfile ? ` (${f.codecProfile})` : ''}`} />
          <Row label="Sample rate" value={`${formatRateKhz(f.sampleRate)} kHz`} />
          <Row
            label="Bit depth"
            tip="Declared by the container, and the number of bits actually used by the samples"
            value={
              f.bitDepth ? (
                <>
                  {f.bitDepth}-bit{f.floatSamples ? ' float' : ''}
                  {m.effectiveBits != null && m.effectiveBits !== f.bitDepth ? <span className="t-amber"> (uses {m.effectiveBits})</span> : null}
                </>
              ) : (
                DASH
              )
            }
          />
          <Row label="Channels" value={`${formatChannels(f.channels)}${f.channelLayout ? ` (${f.channelLayout})` : ''}`} />
          <Row label="Bitrate" value={f.bitrateKbps ? `${formatKbps(f.bitrateKbps)} kbps${f.bitrateMode ? ` ${f.bitrateMode}` : ''}` : DASH} />
          {f.encoder ? <Row label="Encoder" value={f.encoder} /> : null}
          {f.encoderLowpassHz ? <Row label="Encoder lowpass" value={formatHz(f.encoderLowpassHz)} tip="Lowpass frequency recorded by the encoder in the file header" /> : null}
          {f.extensionMismatch ? <Row label="Extension" value={f.extensionMismatch} tone="amber" /> : null}
          <Row label="Size" value={formatBytes(f.fileSize)} />
          <Row label="Duration" value={`${formatDuration(m.duration)}${m.declaredDuration != null && Math.abs(m.declaredDuration - m.duration) > 0.5 ? ` (header says ${formatDuration(m.declaredDuration)})` : ''}`} tone={m.truncated ? 'red' : undefined} />
        </dl>
      </section>
      <section className="tech-block">
        <h3 className="tech-title">Levels</h3>
        <dl>
          <Row label="Sample peak" value={db(m.peakDbfs, 'dBFS', 2)} tip="Highest absolute sample value" />
          {m.channelPeakDbfs.length > 1 ? <Row label="Per channel" value={perChannel(m.channelPeakDbfs, 'dBFS')} /> : null}
          <Row
            label="True peak"
            value={db(m.truePeakDbtp, 'dBTP', 2, true)}
            tone={m.truePeakDbtp > 0 ? 'red' : undefined}
            tip={
              m.truePeakOversampling > 1
                ? `Inter-sample peak, ${m.truePeakOversampling}× oversampled (ITU-R BS.1770-4)`
                : 'At this sample rate inter-sample peaks equal the sample peak; no oversampling is needed (ITU-R BS.1770-4)'
            }
          />
          <Row label="Overs above 0 dBTP" value={m.tpOverEvents > 0 ? `${formatCount(m.tpOverEvents)} events` : 'None'} tone={m.tpOverEvents > 0 ? 'amber' : undefined} />
          <Row label="RMS" value={db(m.rmsDbfs, 'dBFS')} />
          <Row label="Crest factor" value={db(m.crestDb, 'dB')} tip="Peak to RMS ratio; low values mean heavy limiting" />
          <Row label="Peak to loudness" value={db(m.plrDb, 'dB')} tip="True peak minus integrated loudness (PLR)" />
          <Row label="DC offset" value={m.dcOffset.map((v) => `${(v * 100).toFixed(3)}%`).join(' / ') || DASH} />
          <Row
            label="Clipping"
            tone={clip && clip.events > 0 ? 'red' : undefined}
            tip={
              clip || f.lossless
                ? undefined
                : 'Flat runs at the ceiling are not a reliable sign of clipping after lossy decoding; decoded overs and true peak are checked instead.'
            }
            value={
              !clip
                ? f.lossless
                  ? 'Not measured'
                  : 'Not measured for lossy files'
                : clip.events > 0
                  ? `${formatCount(clip.events)} events, ${formatCount(clip.samples)} samples (longest ${clip.longestRun}) at ${formatDb(clip.ceilingDbfs, 2)} dBFS`
                  : 'None detected'
            }
          />
          {m.decodedOvers > 0 ? <Row label="Decoded overs" value={`${formatCount(m.decodedOvers)} samples beyond full scale`} tone="red" /> : null}
        </dl>
      </section>
      <section className="tech-block">
        <h3 className="tech-title">Loudness</h3>
        <dl>
          <Row label="Integrated" value={db(m.integratedLufs, 'LUFS')} tip="EBU R128 / ITU-R BS.1770-4, gated" />
          <Row label="Loudness range" value={m.lra != null ? `${m.lra.toFixed(1)} LU` : DASH} tip="EBU Tech 3342 LRA" />
          <Row label="Max momentary" value={db(m.maxMomentaryLufs, 'LUFS')} />
          <Row label="Max short-term" value={db(m.maxShortTermLufs, 'LUFS')} />
        </dl>
      </section>
      <section className="tech-block">
        <h3 className="tech-title">Frequency content</h3>
        <dl>
          <Row label="Upper limit" value={formatHz(m.bandwidth.upperHz)} tip="Highest frequency with meaningful energy (−50 dB below the 1–8 kHz level)" />
          <Row label="Nyquist" value={formatHz(m.bandwidth.nyquist)} />
          <Row
            label="Steep cutoff"
            value={edge ? `${formatHz(edge.hz)}, ${edge.dropDb.toFixed(0)} dB drop over ${formatHz(edge.transitionHz)}` : 'None'}
            tip="A brickwall drop in the long-term spectrum, typical of lossy encoders and resampling"
          />
          {m.bandwidth.ceilingConsistency != null ? (
            <Row label="Cutoff consistency" value={formatPercent(m.bandwidth.ceilingConsistency)} tip="Share of analysis frames in which the energy above the cutoff stays at least 30 dB below the energy just under it" />
          ) : null}
          {m.bandwidth.ultrasonicRelDb != null ? (
            <Row label="Ultrasonic content" value={`${formatDb(m.bandwidth.ultrasonicRelDb)} dB`} tip="Average level between 26 and 40 kHz relative to 2–20 kHz. Upsampled files have almost nothing there." />
          ) : null}
          {m.bandwidthChanges.length ? (
            <Row label="Bandwidth changes" value={m.bandwidthChanges.map((c) => `${formatTimecode(c.time)}: ${formatHz(c.fromHz)} → ${formatHz(c.toHz)}`).join(', ')} tone="amber" />
          ) : null}
        </dl>
      </section>
      {st ? (
        <section className="tech-block">
          <h3 className="tech-title">Stereo</h3>
          <dl>
            <Row label="Correlation" value={st.correlation.toFixed(2)} tip="+1 mono-like, 0 unrelated, −1 out of phase" tone={st.correlation < 0 ? 'red' : undefined} />
            {st.lowCorrelation != null ? <Row label="Bass correlation" value={st.lowCorrelation.toFixed(2)} tip="Below 120 Hz" /> : null}
            <Row label="Mono fold-down loss" value={`${formatDb(st.monoLossDb)} dB`} tip="Level lost when the channels are summed to mono" />
            <Row label="Side to mid" value={`${formatDb(st.sideToMidDb)} dB`} />
            {st.identical ? <Row label="Channels" value="Identical (dual mono)" /> : null}
            {st.inverted ? <Row label="Channels" value="One channel inverted" tone="red" /> : null}
          </dl>
        </section>
      ) : null}
      <section className="tech-block">
        <h3 className="tech-title">Silence and integrity</h3>
        <dl>
          <Row label="Leading silence" value={`${m.silence.leading.toFixed(2)} s`} />
          <Row label="Trailing silence" value={`${m.silence.trailing.toFixed(2)} s`} />
          <Row label="Dropouts" value={m.silence.dropoutCount > 0 ? formatCount(m.silence.dropoutCount) : 'None'} tone={m.silence.dropoutCount > 0 ? 'red' : undefined} />
          {m.silence.abruptCutoff ? <Row label="Ending" value={`Abrupt, at ${formatTimecode(m.silence.abruptCutoff.start)}`} tone="amber" /> : null}
          <Row label="Decode errors" value={m.decodeErrors > 0 ? formatCount(m.decodeErrors) : 'None'} tone={m.decodeErrors > 0 ? 'red' : undefined} />
          <Row label="Checksum" value={CHECKSUM[m.checksum] ?? m.checksum ?? DASH} tone={m.checksum === 'mismatch' ? 'red' : undefined} tip="FLAC files carry an MD5 of the audio; SONORA compares it with the decoded samples" />
          <Row label="Decoder" value={m.decoder} />
          <Row label="Analysis time" value={`${(m.analysisMs / 1000).toFixed(1)} s`} />
        </dl>
      </section>
    </div>
  );
}

function NotAnalyzed({ d, onRetry }: { d: TrackDetail; onRetry: () => void }) {
  const t = d.track;
  return (
    <div className={`not-analyzed ${t.status === 'failed' ? 'is-failed' : ''}`}>
      <StatusMark track={t} />
      {t.status === 'failed' ? (
        <>
          <p className="selectable">{t.errorMessage ?? 'The file could not be analyzed.'}</p>
          {t.errorCode ? <p className="faint num">Error code: {t.errorCode}</p> : null}
        </>
      ) : t.status === 'pending' ? (
        <p>This track is waiting to be analyzed.</p>
      ) : (
        <p>Analysis in progress.</p>
      )}
      {t.status !== 'analyzing' && d.fileExists ? (
        <Button icon={LuRefreshCw} onClick={onRetry}>
          {t.status === 'failed' ? 'Try again' : 'Analyze now'}
        </Button>
      ) : null}
    </div>
  );
}

export function TrackView({ id }: { id: number }) {
  const { client, settings } = useCore();
  const actions = useTrackActions();
  const player = usePlayer();
  const [focus, setFocus] = useState<Focus>(null);
  const detail = useResource(() => client.track(id), [id], { events: ['library.changed', 'scan.finished'], throttleMs: 1500 });
  // This track finished (re-)analysis, or started it: show the new state now.
  useCoreEvent(['scan.item'], (_n, payload) => {
    if ((payload as ScanItemEvent).trackId === id) detail.reload();
  });
  const d = detail.data;
  const a = d?.analysis ?? null;
  const row = d?.track ?? null;
  const canPlay = !!d?.fileExists;

  // P plays or pauses this track (before the window-wide shortcut, which
  // would otherwise toggle whatever track is loaded).
  const toggle = player.toggle;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || (e.key !== 'p' && e.key !== 'P') || !row || !canPlay) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
      // Not while a dialog is open.
      if (document.querySelector('dialog[open]')) return;
      e.preventDefault();
      toggle(row);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [row, canPlay, toggle]);

  // The core's cutoff (set only for a steep edge), shared with the library's Cutoff column.
  const cutoffHz = d?.track.cutoffHz ?? null;
  const cutoff = useMemo(() => {
    if (!a || cutoffHz == null) return null;
    const concern = a.assessment.findings.some((f) => ['lossy_source_suspected', 'bandwidth_below_bitrate', 'steep_cutoff', 'lossy_codec'].includes(f.id) && f.severity !== 'info');
    return { hz: cutoffHz, concern };
  }, [a, cutoffHz]);

  const back = (
    <IconButton
      label="Back"
      icon={LuArrowLeft}
      onClick={() => {
        if (window.history.length > 1) window.history.back();
        else navigate({ view: 'library' });
      }}
    />
  );

  if (detail.error) {
    return (
      <div className="view view-scroll">
        <ViewHeader back={back} title="Track not found" />
        <ErrorState title="This track could not be loaded" error={detail.error} onRetry={detail.reload} />
      </div>
    );
  }
  if (!d) {
    return (
      <div className="view">
        <ViewHeader back={back} title=" " />
      </div>
    );
  }

  const t = d.track;
  const m = a?.measurements;
  const s = a?.assessment;
  const current = player.track?.id === t.id;
  const playing = current && (player.status === 'playing' || player.status === 'loading');
  const playAt = (time: number) => {
    if (!d.fileExists) return;
    if (current) {
      player.seek(time);
      if (!playing) player.resume();
    } else player.play(t, time);
  };
  const onTime = (time: number) => {
    setFocus({ time, nonce: Date.now() });
    // Following a finding while listening: jump there too.
    if (current) player.seek(time);
  };
  const notation = settings?.keyNotation ?? 'camelot';

  return (
    <div className="view view-scroll track-view">
      <ViewHeader
        back={back}
        title={<span className="selectable">{t.displayTitle}</span>}
        subtitle={<span className="selectable">{[t.artist, t.album].filter(Boolean).join(' – ') || t.fileName}</span>}
        actions={
          <>
            <Button
              variant="primary"
              icon={current && player.status === 'loading' ? LuLoaderCircle : playing ? LuPause : LuPlay}
              className={current && player.status === 'loading' ? 'is-loading' : ''}
              onClick={() => player.toggle(t)}
              disabled={!d.fileExists}
              data-tip="Play or pause (P)"
            >
              {playing ? 'Pause' : 'Play'}
            </Button>
            <Button icon={LuUsb} onClick={() => actions.exportDj([t.id])} disabled={!d.fileExists}>
              Export for DJs…
            </Button>
            <Button icon={LuFolderSearch} onClick={() => void actions.reveal(t.path)} disabled={!d.fileExists}>
              {actions.revealText}
            </Button>
            <Button icon={LuRefreshCw} onClick={() => void actions.reanalyze([t.id])} disabled={!d.fileExists || t.status === 'analyzing'}>
              Re-analyze
            </Button>
            <IconButton label="Export this track as CSV" icon={LuDownload} onClick={() => void actions.exportCsv({ ids: [t.id] }, 1)} />
            <IconButton
              label="Remove from library"
              icon={LuTrash2}
              tone="danger"
              onClick={() =>
                void actions.remove([t.id], `“${t.displayTitle}”`).then((ok) => {
                  if (!ok) return;
                  navigate({ view: 'library' }, true);
                  focusMain();
                })
              }
            />
          </>
        }
      />

      {!d.fileExists ? (
        <div className="banner banner-amber" role="status">
          <LuTriangleAlert aria-hidden />
          <span>
            The file is no longer at <span className="selectable mono-path">{t.path}</span>. These results are from its last analysis; rescanning its folder relinks a
            moved or renamed file.
          </span>
        </div>
      ) : null}

      <dl className="spec-strip">
        <Spec label="Format">{t.format ?? t.ext.toUpperCase()}</Spec>
        <Spec label="Sample rate">
          <span className="num">{t.sampleRate ? `${formatRateKhz(t.sampleRate)} kHz` : DASH}</span>
        </Spec>
        <Spec label="Bit depth">
          <span className="num">{t.bitDepth ? `${t.bitDepth}-bit` : t.lossless === false ? 'Lossy' : DASH}</span>
        </Spec>
        <Spec label="Bitrate">
          <span className="num">{t.bitrateKbps ? `${formatKbps(t.bitrateKbps)} kbps` : DASH}</span>
        </Spec>
        <Spec label="Channels">{formatChannels(t.channels)}</Spec>
        <Spec label="Duration">
          <span className="num">{formatDuration(t.duration)}</span>
        </Spec>
        <Spec label="Size">
          <span className="num">{formatBytes(t.size)}</span>
        </Spec>
        <Spec label="BPM">{t.djBpm != null ? <BpmText bpm={t.djBpm} variable={t.tempoVariable} /> : DASH}</Spec>
        <Spec label="Key">{t.djKey != null ? <KeyBadge keyId={t.djKey} notation={notation} /> : DASH}</Spec>
        <Spec label="Analyzed" tip={a ? `Scoring model ${s?.modelVersion ?? ''}, engine ${m?.engineVersion ?? ''}` : undefined}>
          {t.analyzedAt ? formatRelative(t.analyzedAt) : DASH}
        </Spec>
      </dl>

      {!a || !s || !m ? (
        <NotAnalyzed d={d} onRetry={() => void actions.reanalyze([t.id])} />
      ) : (
        <>
          <section className="score-block" aria-label="Scores">
            <div className="score-meters">
              {SCORE_KINDS.map((k) => (
                <ScoreMeter key={k} score={s.scores[k]} label={SCORE_LABEL[k]} hint={SCORE_HINT[k]} />
              ))}
            </div>
            <div className="verdict">
              <p className="verdict-text selectable">{s.verdict}</p>
              <div className="verdict-conf">
                <span className="faint">Confidence</span>
                <span className={`conf conf-${s.confidenceLabel === 'High' ? 'good' : s.confidenceLabel === 'Medium' ? 'fair' : 'poor'}`}>{s.confidenceLabel}</span>
                <span className="num faint">{s.confidence.toFixed(2)}</span>
              </div>
              {s.confidenceReasons.length ? (
                <ul className="conf-reasons">
                  {s.confidenceReasons.map((r) => (
                    <li key={r} className="faint">
                      {r}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          </section>

          <section className="section">
            <div className="section-head">
              <h2 className="section-title">Tempo, key and genre</h2>
            </div>
            <TrackDj t={t} m={m} onEdit={() => actions.editDj({ ids: [t.id], row: t })} />
          </section>

          <section className="section" aria-label="Signal over time">
            {a.visuals ? (
              <Lanes
                duration={m.duration}
                visuals={a.visuals}
                integratedLufs={m.integratedLufs}
                cutoff={cutoff}
                bandwidthChanges={m.bandwidthChanges}
                dropouts={m.silence.dropouts}
                focus={focus}
                playhead={current}
                onPlayAt={d.fileExists ? playAt : undefined}
              />
            ) : (
              <p className="faint lanes-missing">
                Visualizations were not stored for this analysis (they can be turned off or cleared in Settings). Re-analyze the track to see its waveform,
                spectrogram and loudness.
              </p>
            )}
          </section>

          <div className="track-grid">
            <div className="track-main">
              <section className="section">
                <div className="section-head">
                  <h2 className="section-title">Findings</h2>
                  <span className="faint">
                    {plural(s.findings.filter((f) => f.severity !== 'info').length, 'issue')}, {plural(s.findings.filter((f) => f.severity === 'info').length, 'note')}
                  </span>
                </div>
                <Findings
                  findings={s.findings}
                  onTime={onTime}
                  timesFor={(f) => {
                    if (f.times.length) return f.times;
                    // Clipping and overs are located from the visualization markers.
                    if (f.id === 'clipping' || f.id === 'decoded_overs') return a.visuals?.clipTimes ?? [];
                    if (f.id === 'true_peak_overs') return a.visuals?.overTimes ?? [];
                    return [];
                  }}
                />
              </section>
            </div>
            <div className="track-side">
              <section className="section">
                <div className="section-head">
                  <h2 className="section-title">Sub-scores</h2>
                </div>
                <ul className="subscores">
                  {s.subscores.map((sc) => (
                    <li key={sc.id} className="subscore">
                      <div className="subscore-head">
                        <span className="subscore-label">{sc.label}</span>
                        <ScoreCell score={sc.score} emphasis />
                      </div>
                      <p className="subscore-desc faint">
                        {sc.description}
                        {sc.findings.length ? ` ${sc.findings.length === 1 ? 'One finding' : `${sc.findings.length} findings`} in this area.` : ''}
                      </p>
                    </li>
                  ))}
                </ul>
              </section>
              {a.visuals ? (
                <section className="section">
                  <div className="section-head">
                    <h2 className="section-title">Average spectrum</h2>
                  </div>
                  <SpectrumChart spectrum={a.visuals.spectrum} cutoffHz={cutoff?.hz ?? null} nyquist={m.bandwidth.nyquist} />
                </section>
              ) : null}
              {m.bands.length ? (
                <section className="section">
                  <div className="section-head">
                    <h2 className="section-title">Energy by band</h2>
                  </div>
                  <BandBars bands={m.bands} />
                </section>
              ) : null}
            </div>
          </div>

          <section className="section">
            <div className="section-head">
              <h2 className="section-title">Technical details</h2>
              <span className="faint">
                Scoring model <span className="num">{s.modelVersion}</span>, analyzed {formatDate(a.createdAt)}
              </span>
            </div>
            <TechTables a={a} steepCutoff={t.cutoffHz != null} />
          </section>
        </>
      )}

      {d.history.length > 0 ? (
        <section className="section">
          <div className="section-head">
            <h2 className="section-title">Analysis history</h2>
          </div>
          <ul className="track-history">
            {d.history.map((h) => (
              <li key={`${h.scanId}-${h.startedAt}`}>
                <a className="row-link" href={`#/history/${h.scanId}`}>
                  {formatDate(h.startedAt)}
                </a>
                <span className="faint">{kindLabel(h.kind)}</span>
                <span className={h.outcome === 'failed' ? 't-red' : 'muted'}>{OUTCOME_LABEL[h.outcome] ?? h.outcome}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {actions.dialogs}
    </div>
  );
}
