/**
 * Exports tracks as DJ-ready files: one format every player reads, tempo,
 * key, genre and artwork in the tags, and an optional rekordbox.xml
 * playlist. The originals are only read; new files never overwrite
 * anything.
 */

import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuCircleAlert, LuCircleCheck, LuFolderOpen, LuTriangleAlert } from 'react-icons/lu';
import type { ExportFormat, ExportOptions, ExportPlan, ExportQuality, ExportReport, KeyNotation } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { ProgressBar, Segmented } from '../components/Controls.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { formatBytes, formatCount, plural } from '../lib/format.ts';
import { KEY_NOTATIONS } from '../lib/keys.ts';
import { isAbsolutePath } from '../lib/paths.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { useJobs } from '../state/jobs.tsx';
import { useToast } from '../state/toasts.tsx';
import { revealLabel } from '../lib/platform.ts';

export interface ExportTarget {
  ids: number[];
}

export const FORMATS: { value: ExportFormat; label: string; hint: string; about: string }[] = [
  {
    value: 'aiff',
    label: 'AIFF',
    hint: 'Lossless, full tags',
    about: 'Lossless, with tempo, key, genre and artwork that rekordbox, Serato, Traktor, Engine DJ and CDJs all read. The safest choice.',
  },
  {
    value: 'wav',
    label: 'WAV',
    hint: 'Lossless, tags vary',
    about: 'Lossless and plays everywhere, but some players and older CDJs ignore the tags in WAV files, so tempo, key and artwork may not show.',
  },
  {
    value: 'mp3',
    label: 'MP3 320',
    hint: 'Smallest, plays anywhere',
    about: 'Plays on every device, including old players and car stereos. Lossless sources lose some quality; MP3 sources are copied without re-encoding.',
  },
];

export const QUALITY: { value: ExportQuality; label: string; hint: string; about: string }[] = [
  { value: 'universal', label: '16-bit / 44.1 kHz', hint: 'Every player', about: 'CD quality, readable by every CDJ, controller and USB player.' },
  {
    value: 'high',
    label: 'Up to 24-bit / 48 kHz',
    hint: 'Keeps more resolution',
    about: 'Keeps 24-bit and 48 kHz where the source has them. Some older players cannot read 24-bit files.',
  },
];

function defaultPlaylist(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `SONORA ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export const isFormat = (v: unknown): v is ExportFormat => v === 'aiff' || v === 'wav' || v === 'mp3';
export const isQuality = (v: unknown): v is ExportQuality => v === 'universal' || v === 'high';

type Phase = 'setup' | 'running' | 'done';

export function DjExportDialog({ target, onClose }: { target: ExportTarget | null; onClose: () => void }) {
  const { client, bridge, settings } = useCore();
  const jobs = useJobs();
  const toast = useToast();
  const id = useId();
  const [format, setFormat] = useState<ExportFormat>(() => readPref('djexport.format', 'aiff', isFormat));
  const [quality, setQuality] = useState<ExportQuality>(() => readPref('djexport.quality', 'universal', isQuality));
  const [notation, setNotation] = useState<KeyNotation>(settings?.keyNotation ?? 'camelot');
  const [bestCopy, setBestCopy] = useState(true);
  const [xml, setXml] = useState(true);
  const [playlist, setPlaylist] = useState(defaultPlaylist);
  const [folder, setFolder] = useState<string>(() => readPref('djexport.folder', '', (v): v is string => typeof v === 'string'));
  const [plan, setPlan] = useState<ExportPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>('setup');
  const [report, setReport] = useState<ExportReport | null>(null);
  const [starting, setStarting] = useState(false);
  const [showItems, setShowItems] = useState(false);
  // Plans again when another export finishes while this one is set up.
  const [planTick, setPlanTick] = useState(0);
  // Set before the start call: a short export may finish before it returns.
  const awaiting = useRef(false);
  const open = target != null;

  const options = useMemo<ExportOptions>(() => ({ format, quality, keyNotation: notation, bestCopy, rekordboxXml: xml }), [format, quality, notation, bestCopy, xml]);

  // A new export starts from the setup page.
  const targetKey = target ? target.ids.join(',') : '';
  useEffect(() => {
    if (!target) return;
    setPhase('setup');
    awaiting.current = false;
    setReport(null);
    setPlan(null);
    setPlanError(null);
    setShowItems(false);
    setPlaylist(defaultPlaylist());
    setNotation(settings?.keyNotation ?? 'camelot');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey]);

  // While it shows a running export, the dialog reports the outcome itself.
  const watch = jobs.watchExport;
  const watching = open && phase === 'running';
  useEffect(() => (watching ? watch() : undefined), [watching, watch]);

  // What would happen with these options.
  useEffect(() => {
    if (!target || phase !== 'setup') return;
    let stale = false;
    const t = setTimeout(() => {
      client.exportPlan(target.ids, options).then(
        (p) => {
          if (!stale) {
            setPlan(p);
            setPlanError(null);
          }
        },
        (e: unknown) => {
          if (!stale) setPlanError(e instanceof Error ? e.message : String(e));
        },
      );
    }, 120);
    return () => {
      stale = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey, options, phase, client, planTick]);

  useCoreEvent(['djexport.finished'], (_n, payload) => {
    if (!open) return;
    if (!awaiting.current) {
      // Someone else's export ended: this one may start now.
      setPlanTick((n) => n + 1);
      return;
    }
    awaiting.current = false;
    setReport(payload as ExportReport);
    setPhase('done');
  });

  // The dialog element stays mounted while closed, so closing it returns
  // focus to where it was.
  if (!target) return <Dialog open={false} title="" onClose={onClose}>{null}</Dialog>;

  const chooseFolder = async () => {
    const f = await bridge.pickFolder('Choose where to export');
    if (f) {
      setFolder(f);
      writePref('djexport.folder', f);
    }
  };

  const start = async () => {
    const dest = folder.trim();
    if (!isAbsolutePath(dest)) {
      toast({ tone: 'warning', title: 'Choose a folder to export to' });
      return;
    }
    setStarting(true);
    writePref('djexport.format', format);
    writePref('djexport.quality', quality);
    writePref('djexport.folder', dest);
    // Running before the call returns: its finish event may come first.
    awaiting.current = true;
    setPhase('running');
    jobs.started('export');
    try {
      await client.exportStart(target.ids, options, dest, playlist.trim() || defaultPlaylist());
    } catch (e) {
      awaiting.current = false;
      setPhase('setup');
      jobs.resync();
      toast({ tone: 'error', title: 'The export could not start', body: e instanceof Error ? e.message : String(e) });
    } finally {
      setStarting(false);
    }
  };

  const fmt = FORMATS.find((f) => f.value === format);
  const q = QUALITY.find((x) => x.value === quality);
  const exportable = plan?.exported ?? 0;
  const problems = plan?.items.filter((i) => i.skip || i.warnings.length || i.betterCopy) ?? [];
  const busyElsewhere = plan?.running && phase === 'setup';

  let body: ReactNode;
  let footer: ReactNode;
  if (phase === 'setup') {
    body = (
      <div className="xp">
        <p className="dialog-text">
          {plural(target.ids.length, 'track')} become files ready for DJ software and players, with tempo, key, genre and artwork in their tags. Your original files are
          only read.
        </p>
        <div className="xp-grid">
          <span className="xp-label" id={`${id}-fmt`}>
            Format
          </span>
          <div>
            <Segmented label="Format" value={format} onChange={setFormat} options={FORMATS.map((f) => ({ value: f.value, label: f.label, hint: f.hint }))} />
            <p className="xp-about faint">{fmt?.about}</p>
          </div>
          {format !== 'mp3' ? (
            <>
              <span className="xp-label">Quality</span>
              <div>
                <Segmented label="Quality" value={quality} onChange={setQuality} options={QUALITY.map((x) => ({ value: x.value, label: x.label, hint: x.hint }))} />
                <p className="xp-about faint">{q?.about}</p>
              </div>
            </>
          ) : null}
          <span className="xp-label">Key in tags</span>
          <div>
            <Segmented
              label="Key notation in tags"
              size="sm"
              value={notation}
              onChange={setNotation}
              options={KEY_NOTATIONS.map((n) => ({ value: n.id, label: n.example, hint: `${n.label}: ${n.description}` }))}
            />
            <p className="xp-about faint">{KEY_NOTATIONS.find((n) => n.id === notation)?.label}. Use the notation your DJ software shows.</p>
          </div>
          <span className="xp-label">Folder</span>
          <div className="xp-folder">
            {bridge.native ? (
              <>
                <Button size="sm" icon={LuFolderOpen} onClick={() => void chooseFolder()}>
                  Choose folder…
                </Button>
                <span className={`mono-path ${folder ? '' : 'faint'}`}>{folder || 'No folder chosen'}</span>
              </>
            ) : (
              <input
                className="text-input selectable xp-path"
                placeholder="/full/path/to/a/folder"
                aria-label="Export folder (full path)"
                value={folder}
                onChange={(e) => setFolder(e.target.value)}
              />
            )}
          </div>
        </div>
        <div className="xp-checks">
          <label className="check-row">
            <input type="checkbox" className="check" checked={bestCopy} onChange={(e) => setBestCopy(e.target.checked)} />
            <span>
              Use the best copy of each recording <span className="faint">(when the library holds the same track in several files)</span>
            </span>
          </label>
          <label className="check-row">
            <input type="checkbox" className="check" checked={xml} onChange={(e) => setXml(e.target.checked)} />
            <span>Write a rekordbox.xml playlist</span>
          </label>
          {xml ? (
            <label className="xp-playlist">
              <span className="faint">Playlist name</span>
              <input className="text-input" value={playlist} maxLength={120} onChange={(e) => setPlaylist(e.target.value)} />
            </label>
          ) : null}
        </div>
        <div className="xp-plan" aria-live="polite">
          {planError ? (
            <p className="t-red">{planError}</p>
          ) : !plan ? (
            <p className="faint">Checking the tracks…</p>
          ) : (
            <>
              <p>
                <strong>
                  <span className="num">{formatCount(exportable)}</span> {exportable === 1 ? 'file' : 'files'}
                </strong>{' '}
                will be written
                {plan.betterCopies ? `, ${formatCount(plan.betterCopies)} from a better copy` : ''}
                {plan.skipped ? `; ${formatCount(plan.skipped)} skipped` : ''}.
                {plan.withWarnings ? <span className="t-amber"> {plural(plan.withWarnings, 'file')} with notes.</span> : null}
              </p>
              {plan.problem ? (
                <p className="xp-problem">
                  <LuCircleAlert aria-hidden /> {plan.problem}
                </p>
              ) : null}
              {busyElsewhere ? <p className="t-amber">Another export is running; wait for it to finish.</p> : null}
              {problems.length ? (
                <>
                  <button type="button" className="link" aria-expanded={showItems} onClick={() => setShowItems(!showItems)}>
                    {showItems ? 'Hide details' : 'Show details'}
                  </button>
                  {showItems ? (
                    <ul className="xp-items">
                      {problems.map((i) => (
                        <li key={i.trackId}>
                          <span className="xp-item-title">{i.artist ? `${i.artist} – ${i.title}` : i.title}</span>
                          {i.skip ? <span className="faint">Skipped: {i.skip === 'missing' ? 'the file is missing' : 'another selected track is the same recording'}</span> : null}
                          {i.betterCopy ? <span className="faint">From a better copy ({i.sourceFormat ?? 'another file'})</span> : null}
                          {i.warnings.map((w) => (
                            <span key={w.code} className="t-amber">
                              {w.message}
                            </span>
                          ))}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </>
              ) : null}
            </>
          )}
        </div>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" busy={starting} disabled={!plan || exportable === 0 || !!plan.problem || !!busyElsewhere || !folder.trim()} onClick={() => void start()}>
          {exportable > 0 ? `Export ${plural(exportable, 'file')}` : 'Export'}
        </Button>
      </>
    );
  } else if (phase === 'running') {
    const p = jobs.exportProgress;
    body = (
      <div className="xp-run" aria-live="polite">
        <p>
          {p ? (
            <>
              Exporting <span className="num">{formatCount(Math.min(p.done + 1, p.total))}</span> of <span className="num">{formatCount(p.total)}</span>: <span className="faint">{p.current}</span>
            </>
          ) : (
            'Starting the export…'
          )}
        </p>
        <ProgressBar value={p?.fraction ?? 0} label="Export progress" />
        <p className="faint">You can close this window; the export continues and SONORA tells you when it is done.</p>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" className="is-danger-text" onClick={jobs.cancelExport}>
          Stop export
        </Button>
        <Button onClick={onClose}>Close</Button>
      </>
    );
  } else {
    body = <ExportResult report={report} />;
    const first = report?.rekordboxXml ?? report?.written?.[0]?.path;
    footer = (
      <>
        {first ? (
          <Button icon={LuFolderOpen} onClick={() => void bridge.reveal(first).catch(() => undefined)}>
            {revealLabel(bridge.platform)}
          </Button>
        ) : null}
        <Button variant="primary" onClick={onClose} data-autofocus>
          Done
        </Button>
      </>
    );
  }

  return (
    <Dialog open title={phase === 'done' ? 'Export finished' : 'Export for DJ software'} onClose={onClose} width={620} footer={footer}>
      {body}
    </Dialog>
  );
}

function ExportResult({ report }: { report: ExportReport | null }) {
  if (!report) return null;
  const written = report.written ?? [];
  const failed = report.failed ?? [];
  const skipped = report.skipped ?? [];
  const bytes = written.reduce((s, w) => s + w.bytes, 0);
  return (
    <div className="xp-done">
      {report.error ? (
        <p className="xp-status t-red">
          <LuCircleAlert aria-hidden /> {report.error}
        </p>
      ) : (
        <p className={`xp-status ${failed.length || report.cancelled ? 't-amber' : 't-lime'}`}>
          {failed.length || report.cancelled ? <LuTriangleAlert aria-hidden /> : <LuCircleCheck aria-hidden />}
          {report.cancelled ? 'Stopped after ' : 'Exported '}
          {plural(written.length, 'file')} ({formatBytes(bytes)})
        </p>
      )}
      <p className="mono-path selectable">{report.folder}</p>
      {report.rekordboxXml ? <p className="faint">Playlist: rekordbox.xml in the same folder.</p> : null}
      {report.xmlError ? <p className="t-amber">{report.xmlError}</p> : null}
      {failed.length ? (
        <div className="xp-list">
          <p className="t-red">{plural(failed.length, 'track')} could not be exported:</p>
          <ul>
            {failed.map((f) => (
              <li key={f.trackId}>
                {f.title}: <span className="faint selectable">{f.error}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {skipped.length ? <p className="faint">{plural(skipped.length, 'track')} skipped (missing files, or the same recording twice).</p> : null}
      {written.length ? (
        <div className="xp-howto">
          <h3 className="xp-howto-title">Bring them into your DJ software</h3>
          <dl>
            <div>
              <dt>rekordbox</dt>
              <dd>
                {report.rekordboxXml
                  ? 'In Preferences → Advanced → Database, choose this rekordbox.xml under “rekordbox xml”. The playlist then appears under “rekordbox xml” in the tree: right-click it and choose Import Playlist.'
                  : 'Drag the folder into the collection. rekordbox reads the tempo, key, genre and artwork from the tags.'}
              </dd>
            </div>
            <div>
              <dt>Serato DJ</dt>
              <dd>Open Files, find the folder and drag it onto the crate list. Serato reads the tempo, key and genre from the tags.</dd>
            </div>
            <div>
              <dt>Traktor, Engine DJ, VirtualDJ</dt>
              <dd>Add the folder to the collection; the tags carry the tempo, key, genre and artwork.</dd>
            </div>
            <div>
              <dt>USB stick</dt>
              <dd>Copy the folder to the stick. For CDJ track browsing with waveforms and hot cues, export the stick from rekordbox after importing.</dd>
            </div>
          </dl>
        </div>
      ) : null}
    </div>
  );
}
