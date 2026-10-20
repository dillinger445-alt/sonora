/**
 * Sending playlists to a USB drive: pick the drive and the playlists, see
 * what will happen (and whether it fits), send, and read how to use the
 * result in DJ software and on players.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuCircleAlert, LuCircleCheck, LuFolder, LuFolderOpen, LuHardDrive, LuListFilter, LuListMusic, LuRefreshCw, LuTriangleAlert, LuUsb } from 'react-icons/lu';
import type { Drive, ExportFormat, ExportQuality, KeyNotation, PlaylistNode, UsbOptions, UsbPlan, UsbReport } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { ProgressBar, Segmented } from '../components/Controls.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { formatBytes, formatCount, plural } from '../lib/format.ts';
import { KEY_NOTATIONS } from '../lib/keys.ts';
import { isAbsolutePath } from '../lib/paths.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { revealLabel } from '../lib/platform.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { describeUsb, useJobs } from '../state/jobs.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import { useToast } from '../state/toasts.tsx';
import { FORMATS, isFormat, isQuality, QUALITY } from './DjExportDialog.tsx';

type Phase = 'setup' | 'running' | 'done';

const isString = (v: unknown): v is string => typeof v === 'string';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/** Used, about to be written, and free space of a drive, as a bar. */
function CapacityBar({ total, free, write }: { total: number | null; free: number | null; write: number }) {
  if (!total || free == null) return <div className="cap cap-unknown" aria-hidden />;
  const used = Math.max(0, total - free);
  const w = Math.min(write, free);
  const pct = (v: number) => `${Math.max(0, Math.min(100, (v / total) * 100))}%`;
  return (
    <div className={`cap ${write > free ? 'is-over' : ''}`} aria-hidden>
      <span className="cap-used" style={{ width: pct(used) }} />
      <span className="cap-write" style={{ left: pct(used), width: pct(w) }} />
    </div>
  );
}

function DriveCard({ d, selected, write, onSelect }: { d: Drive; selected: boolean; write: number; onSelect: () => void }) {
  const Icon = d.removable ? LuUsb : LuHardDrive;
  return (
    <button type="button" role="radio" aria-checked={selected} className={`drive ${selected ? 'is-on' : ''}`} onClick={onSelect}>
      <span className="drive-head">
        <Icon aria-hidden className="drive-icon" />
        <span className="drive-name ellipsis">{d.name}</span>
        {d.fsLabel ? <span className="drive-fs">{d.fsLabel}</span> : null}
      </span>
      <CapacityBar total={d.total} free={d.free} write={selected ? write : 0} />
      <span className="drive-space faint num">{d.free != null && d.total ? `${formatBytes(d.free)} free of ${formatBytes(d.total)}` : 'Space unknown'}</span>
    </button>
  );
}

/** The playlist tree with checkboxes; a checked folder includes all inside. */
function PlaylistPicker({ selected, onChange }: { selected: ReadonlySet<number>; onChange: (s: Set<number>) => void }) {
  const pl = usePlaylists();
  const rows: { n: PlaylistNode; depth: number; implied: boolean }[] = [];
  const walk = (parent: number | null, depth: number, implied: boolean) => {
    for (const n of pl.childrenOf(parent)) {
      rows.push({ n, depth, implied });
      if (n.kind === 'folder') walk(n.id, depth + 1, implied || selected.has(n.id));
    }
  };
  walk(null, 0, false);
  if (!rows.length) return <p className="faint">No playlists yet. Make some in the sidebar first.</p>;
  return (
    <ul className="pick" role="group" aria-label="Playlists to send">
      {rows.map(({ n, depth, implied }) => {
        const Icon = n.kind === 'folder' ? LuFolder : n.kind === 'smart' ? LuListFilter : LuListMusic;
        const on = implied || selected.has(n.id);
        return (
          <li key={n.id} style={{ paddingLeft: depth * 18 }}>
            <label className={`check-row pick-row ${implied ? 'is-implied' : ''}`}>
              <input
                type="checkbox"
                className="check"
                checked={on}
                disabled={implied}
                onChange={(e) => {
                  const next = new Set(selected);
                  if (e.target.checked) next.add(n.id);
                  else next.delete(n.id);
                  // A checked folder covers what is inside it.
                  if (n.kind === 'folder' && e.target.checked) for (const x of pl.nodes) if (x.id !== n.id && pl.isWithin(x.id, n.id)) next.delete(x.id);
                  onChange(next);
                }}
              />
              <Icon aria-hidden className="pick-icon" />
              <span className="ellipsis">{n.name}</span>
              <span className="num faint pick-count">{n.kind === 'folder' ? plural(n.children, 'item') : formatCount(n.count)}</span>
            </label>
          </li>
        );
      })}
    </ul>
  );
}

/** The folders and files the plan shows, grouped by folder. */
function Preview({ plan }: { plan: UsbPlan }) {
  const groups = useMemo(() => {
    const m = new Map<string, UsbPlan['preview']>();
    for (const f of plan.preview) {
      const dir = f.path.slice(0, f.path.lastIndexOf('/'));
      m.set(dir, [...(m.get(dir) ?? []), f]);
    }
    return [...m.entries()];
  }, [plan.preview]);
  const label: Record<string, string> = { new: 'new', update: 'updated', rename: 'moved', keep: '' };
  return (
    <div className="usb-preview" aria-label="Files on the drive" role="region">
      {groups.map(([dir, files]) => {
        const p = plan.playlists.find((x) => x.dir === dir);
        return (
          <div key={dir} className="usb-dir">
            <div className="usb-dir-name">
              <LuFolderOpen aria-hidden />
              <span className="mono-path">{dir}</span>
            </div>
            <ul>
              {files.map((f) => (
                <li key={f.path} className={`usb-file is-${f.action}`}>
                  <span className="ellipsis">{f.path.slice(dir.length + 1)}</span>
                  {label[f.action] ? <span className="usb-tag">{label[f.action]}</span> : null}
                </li>
              ))}
              {p && p.tracks > files.length ? <li className="usb-file faint">and {formatCount(p.tracks - files.length)} more</li> : null}
            </ul>
          </div>
        );
      })}
    </div>
  );
}

function summary(plan: UsbPlan): string {
  const f = plan.files;
  const parts: string[] = [];
  const writing = f.new + f.update;
  if (writing) parts.push(`${plural(writing, 'file')} to write (about ${formatBytes(plan.bytes.write)})`);
  if (f.rename) parts.push(`${formatCount(f.rename)} to move`);
  if (f.keep) parts.push(`${formatCount(f.keep)} already there`);
  if (!parts.length) return f.total ? 'Everything is on the drive already.' : 'Nothing to send.';
  return `${parts.join(', ')}.`;
}

export function UsbDialog({ playlistIds, onClose }: { playlistIds: number[] | null; onClose: () => void }) {
  const { client, bridge, settings } = useCore();
  const jobs = useJobs();
  const toast = useToast();
  const open = playlistIds != null;
  const [phase, setPhase] = useState<Phase>('setup');
  const [drives, setDrives] = useState<Drive[] | null>(null);
  const [root, setRoot] = useState<string>('');
  const [custom, setCustom] = useState<string>('');
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [format, setFormat] = useState<ExportFormat>(() => readPref('usb.format', 'aiff', isFormat));
  const [quality, setQuality] = useState<ExportQuality>(() => readPref('usb.quality', 'universal', isQuality));
  const [notation, setNotation] = useState<KeyNotation>(settings?.keyNotation ?? 'camelot');
  const [xml, setXml] = useState(() => readPref('usb.xml', true, isBool));
  const [serato, setSerato] = useState(() => readPref('usb.serato', true, isBool));
  const [m3u, setM3u] = useState(() => readPref('usb.m3u', true, isBool));
  const [numbered, setNumbered] = useState(() => readPref('usb.numbered', true, isBool));
  const [removeStale, setRemoveStale] = useState(false);
  const [plan, setPlan] = useState<UsbPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [planning, setPlanning] = useState(false);
  const [report, setReport] = useState<UsbReport | null>(null);
  const [starting, setStarting] = useState(false);
  const awaiting = useRef(false);
  // Set once the user picks a destination, which the drive list arriving
  // later must not undo.
  const chosen = useRef(false);
  const choose = (path: string) => {
    chosen.current = true;
    setRoot(path);
  };

  const loadDrives = useCallback(async () => {
    setDrives(null);
    try {
      const r = await client.drives();
      setDrives(r.drives);
      return r.drives;
    } catch {
      setDrives([]);
      return [];
    }
  }, [client]);

  // A fresh start each time it opens.
  const openKey = playlistIds ? playlistIds.join(',') : null;
  useEffect(() => {
    if (openKey == null) return;
    setPhase('setup');
    setReport(null);
    setPlan(null);
    setPlanError(null);
    setRemoveStale(false);
    setSelected(new Set(playlistIds ?? []));
    setNotation(settings?.keyNotation ?? 'camelot');
    awaiting.current = false;
    chosen.current = false;
    const last = readPref('usb.root', '', isString);
    const lastWasFolder = readPref('usb.rootIsFolder', false, isBool);
    void loadDrives().then((ds) => {
      if (chosen.current) return;
      // The folder sent to last time, or the drive if it is connected,
      // else the first drive.
      if (last && lastWasFolder) {
        setCustom(last);
        setRoot(last);
        return;
      }
      setRoot(ds.some((d) => d.path === last) ? last : (ds[0]?.path ?? ''));
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openKey]);

  const watch = jobs.watchUsb;
  const watching = open && phase === 'running';
  useEffect(() => (watching ? watch() : undefined), [watching, watch]);

  const options = useMemo<UsbOptions>(
    () => ({ format, quality, keyNotation: notation, bestCopy: true, rekordboxXml: xml, m3u, serato, removeStale, numberFolders: numbered }),
    [format, quality, notation, xml, m3u, serato, removeStale, numbered],
  );
  const ids = useMemo(() => [...selected], [selected]);
  const target = root.trim();

  // What sending would do, as choices change.
  const planKey = JSON.stringify([target, ids, options]);
  useEffect(() => {
    if (!open || phase !== 'setup') return;
    if (!target || !isAbsolutePath(target) || ids.length === 0) {
      setPlan(null);
      setPlanError(null);
      return;
    }
    let stale = false;
    setPlanning(true);
    const t = setTimeout(() => {
      client.usbPlan(target, ids, options).then(
        (p) => {
          if (stale) return;
          setPlan(p);
          setPlanError(null);
          setPlanning(false);
        },
        (e: unknown) => {
          if (stale) return;
          setPlan(null);
          setPlanError(e instanceof Error ? e.message : String(e));
          setPlanning(false);
        },
      );
    }, 220);
    return () => {
      stale = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, phase, planKey]);

  useCoreEvent(['usb.finished'], (_n, payload) => {
    if (!open || !awaiting.current) return;
    awaiting.current = false;
    setReport(payload as UsbReport);
    setPhase('done');
  });

  const chooseFolder = async () => {
    let f: string | null = null;
    try {
      f = await bridge.pickFolder('Choose where to send the playlists');
    } catch (e) {
      toast({ tone: 'error', title: 'Could not open the folder dialog', body: e instanceof Error ? e.message : String(e) });
      return;
    }
    if (f) {
      setCustom(f);
      choose(f);
    }
  };

  const start = async () => {
    if (!plan) return;
    setStarting(true);
    writePref('usb.root', target);
    writePref('usb.rootIsFolder', !drives?.some((d) => d.path === target));
    writePref('usb.format', format);
    writePref('usb.quality', quality);
    writePref('usb.xml', xml);
    writePref('usb.serato', serato);
    writePref('usb.m3u', m3u);
    writePref('usb.numbered', numbered);
    awaiting.current = true;
    setPhase('running');
    jobs.started('usb');
    try {
      await client.usbStart(target, ids, options);
    } catch (e) {
      awaiting.current = false;
      setPhase('setup');
      jobs.resync();
      toast({ tone: 'error', title: 'Could not start sending', body: e instanceof Error ? e.message : String(e) });
    } finally {
      setStarting(false);
    }
  };

  const drive = drives?.find((d) => d.path === target);
  const destName = drive?.name ?? (target ? target.split(/[\\/]/).filter(Boolean).pop() : '') ?? '';
  const busyElsewhere = plan?.running && phase === 'setup';
  const fmt = FORMATS.find((f) => f.value === format);
  const q = QUALITY.find((x) => x.value === quality);

  let body: ReactNode;
  let footer: ReactNode;
  if (phase === 'setup') {
    body = (
      <div className="usb">
        <section className="usb-section" aria-label="Where to">
          <div className="usb-section-head">
            <h3 className="usb-h">Drive</h3>
            <Button
              size="sm"
              variant="ghost"
              icon={LuRefreshCw}
              onClick={() =>
                void loadDrives().then((ds) => {
                  // A drive connected just now is the one to use when none was chosen.
                  if (!target && ds[0]) setRoot(ds[0].path);
                })
              }
            >
              Look again
            </Button>
          </div>
          <div className="drives" role="radiogroup" aria-label="Drive">
            {drives == null ? <p className="faint">Looking for drives…</p> : null}
            {drives?.map((d) => (
              <DriveCard key={d.path} d={d} selected={target === d.path} write={plan ? Math.max(0, plan.bytes.write - plan.bytes.freed) : 0} onSelect={() => choose(d.path)} />
            ))}
            {custom ? (
              <button type="button" role="radio" aria-checked={target === custom} className={`drive ${target === custom ? 'is-on' : ''}`} onClick={() => choose(custom)}>
                <span className="drive-head">
                  <LuFolder aria-hidden className="drive-icon" />
                  <span className="drive-name ellipsis">{custom.split(/[\\/]/).filter(Boolean).pop() ?? custom}</span>
                </span>
                <span className="drive-space faint mono-path ellipsis">{custom}</span>
              </button>
            ) : null}
          </div>
          {drives && drives.length === 0 && !custom ? <p className="faint usb-none">No USB drive found. Connect one and choose Look again, or send to a folder.</p> : null}
          {bridge.native ? (
            <Button size="sm" icon={LuFolderOpen} onClick={() => void chooseFolder()}>
              Send to a folder…
            </Button>
          ) : (
            <input
              className="text-input selectable"
              placeholder="/full/path/to/a/drive/or/folder"
              aria-label="Drive or folder (full path)"
              value={custom}
              onChange={(e) => {
                setCustom(e.target.value);
                choose(e.target.value);
              }}
            />
          )}
        </section>
        <div className="usb-cols">
          <section className="usb-section" aria-label="Playlists">
            <h3 className="usb-h">Playlists</h3>
            <PlaylistPicker selected={selected} onChange={setSelected} />
          </section>
          <section className="usb-section" aria-label="Files">
            <h3 className="usb-h">Files</h3>
            <Segmented label="Format" value={format} onChange={setFormat} options={FORMATS.map((f) => ({ value: f.value, label: f.label, hint: f.hint }))} />
            <p className="xp-about faint">{fmt?.about}</p>
            {format !== 'mp3' ? (
              <>
                <Segmented label="Quality" size="sm" value={quality} onChange={setQuality} options={QUALITY.map((x) => ({ value: x.value, label: x.label, hint: x.hint }))} />
                <p className="xp-about faint">{q?.about}</p>
              </>
            ) : null}
            <Segmented
              label="Key notation in tags"
              size="sm"
              value={notation}
              onChange={setNotation}
              options={KEY_NOTATIONS.map((n) => ({ value: n.id, label: n.example, hint: `${n.label}: ${n.description}` }))}
            />
            <p className="xp-about faint">Key in the tags: {KEY_NOTATIONS.find((n) => n.id === notation)?.label ?? 'Camelot'}. Use the notation your DJ software shows.</p>
            <div className="usb-checks">
              <label className="check-row">
                <input type="checkbox" className="check" checked={xml} onChange={(e) => setXml(e.target.checked)} />
                <span>rekordbox.xml with all the playlists</span>
              </label>
              <label className={`check-row ${plan && !plan.seratoPossible ? 'is-disabled' : ''}`}>
                <input type="checkbox" className="check" checked={serato && (plan?.seratoPossible ?? true)} disabled={plan != null && !plan.seratoPossible} onChange={(e) => setSerato(e.target.checked)} />
                <span>
                  Serato crates {plan && !plan.seratoPossible ? <span className="faint">(only when sending to the top of a drive)</span> : null}
                </span>
              </label>
              <label className="check-row">
                <input type="checkbox" className="check" checked={m3u} onChange={(e) => setM3u(e.target.checked)} />
                <span>An M3U playlist in each folder</span>
              </label>
              <label className="check-row">
                <input type="checkbox" className="check" checked={numbered} onChange={(e) => setNumbered(e.target.checked)} />
                <span>Number the folders in sidebar order</span>
              </label>
            </div>
          </section>
        </div>
        <section className="usb-plan" aria-label="What happens">
          {/* Only the outcome is announced as it changes, not the file list. */}
          <div className="usb-status" aria-live="polite">
            {!target ? (
              <p className="faint">Choose a drive.</p>
            ) : ids.length === 0 ? (
              <p className="faint">Choose the playlists to send.</p>
            ) : planError ? (
              <p className="xp-problem">
                <LuCircleAlert aria-hidden /> {planError}
              </p>
            ) : !plan ? (
              <p className="faint">{planning ? 'Checking the drive…' : ''}</p>
            ) : (
              <>
                <p className="usb-summary">
                  <strong>{summary(plan)}</strong>
                  {plan.bytes.free != null ? <span className="faint"> {formatBytes(plan.bytes.free)} free on the drive.</span> : null}
                </p>
                {plan.problem ? (
                  <p className="xp-problem">
                    <LuCircleAlert aria-hidden /> {plan.problem}
                  </p>
                ) : null}
                {busyElsewhere ? <p className="t-amber">An export or another send is running; wait for it to finish.</p> : null}
              </>
            )}
          </div>
          {target && ids.length > 0 && !planError && plan ? (
            <>
              {plan.files.stale > 0 ? (
                <label className="check-row usb-stale">
                  <input type="checkbox" className="check" checked={removeStale} onChange={(e) => setRemoveStale(e.target.checked)} />
                  <span>
                    Remove the {plural(plan.files.stale, 'file')} ({formatBytes(plan.files.staleBytes)}) SONORA put there for tracks no longer in these playlists, or for
                    playlists you deleted.{' '}
                    <span className="faint">They have not changed since. Nothing else on the drive is ever removed.</span>
                  </span>
                </label>
              ) : null}
              {plan.warnings.length ? (
                <ul className="usb-warnings">
                  {plan.warnings.slice(0, 4).map((w) => (
                    <li key={w} className="t-amber">
                      <LuTriangleAlert aria-hidden /> {w}
                    </li>
                  ))}
                  {plan.warnings.length > 4 ? <li className="faint">and {plural(plan.warnings.length - 4, 'more note')}</li> : null}
                </ul>
              ) : null}
              {plan.preview.length ? <Preview plan={plan} /> : null}
            </>
          ) : null}
        </section>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" icon={LuUsb} busy={starting} disabled={!plan || !!plan.problem || plan.files.total === 0 || !!busyElsewhere || planning} onClick={() => void start()}>
          {destName ? `Send to ${destName}` : 'Send'}
        </Button>
      </>
    );
  } else if (phase === 'running') {
    const p = jobs.usbProgress;
    body = (
      <div className="xp-run" aria-live="polite">
        <p>
          {p && p.phase === 'writing' && p.total > 0 ? (
            <>
              Writing <span className="num">{formatCount(Math.min(p.done + 1, p.total))}</span> of <span className="num">{formatCount(p.total)}</span>: <span className="faint">{p.current}</span>
            </>
          ) : (
            'Getting the drive ready…'
          )}
        </p>
        <ProgressBar value={p?.fraction ?? 0} label="Sending progress" />
        <p className="faint">Keep the drive connected. You can close this window; SONORA tells you when it is done.</p>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" className="is-danger-text" onClick={jobs.cancelUsb}>
          Stop
        </Button>
        <Button onClick={onClose}>Close</Button>
      </>
    );
  } else {
    const d = report ? describeUsb(report) : null;
    body = report ? (
      <div className="xp-done">
        <p className={`xp-status ${d?.tone === 'success' ? 't-lime' : d?.tone === 'error' ? 't-red' : 't-amber'}`}>
          {d?.tone === 'success' ? <LuCircleCheck aria-hidden /> : <LuTriangleAlert aria-hidden />}
          {d?.title}
        </p>
        {d?.body ? <p>{d.body}.</p> : null}
        <p className="mono-path selectable">{report.folder}</p>
        {(report.failed ?? []).length ? (
          <div className="xp-list">
            <p className="t-red">{plural((report.failed ?? []).length, 'track')} could not be written:</p>
            <ul>
              {(report.failed ?? []).map((f) => (
                <li key={f.trackId}>
                  {f.title}: <span className="faint selectable">{f.error}</span>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {(report.warnings ?? []).length ? (
          <ul className="usb-warnings">
            {(report.warnings ?? []).slice(0, 5).map((w) => (
              <li key={w} className="t-amber">
                <LuTriangleAlert aria-hidden /> {w}
              </li>
            ))}
          </ul>
        ) : null}
        <div className="xp-howto">
          <h3 className="xp-howto-title">Using the drive</h3>
          <dl>
            <div>
              <dt>CDJs, XDJs, controllers</dt>
              <dd>Browse the drive's folders: each playlist is a folder, its tracks numbered in set order.</dd>
            </div>
            {report.rekordboxXml ? (
              <div>
                <dt>rekordbox</dt>
                <dd>
                  In Preferences → Advanced → Database, choose SONORA/rekordbox.xml on the drive under “rekordbox xml”. The playlists appear under “rekordbox xml”: right-click one and choose Import Playlist. To export the drive for CDJs with waveforms, do it from rekordbox afterwards.
                </dd>
              </div>
            ) : null}
            {report.crates ? (
              <div>
                <dt>Serato DJ</dt>
                <dd>The crates are in the drive's _Serato_ folder; Serato DJ lists them under the drive when it is connected.</dd>
              </div>
            ) : null}
            {report.m3u ? (
              <div>
                <dt>Engine DJ, Traktor, VirtualDJ</dt>
                <dd>Open the M3U playlist in a playlist's folder, or add the folder to the collection.</dd>
              </div>
            ) : null}
          </dl>
          <p className="faint">Eject the drive in Finder or File Explorer before unplugging it.</p>
        </div>
      </div>
    ) : null;
    footer = (
      <>
        {report?.folder && !report.error ? (
          <Button icon={LuFolderOpen} onClick={() => void bridge.reveal(report.folder).catch(() => undefined)}>
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
    <Dialog open={open} title={phase === 'done' ? 'Sent to the drive' : 'Send playlists to a USB drive'} onClose={onClose} width={780} footer={footer}>
      {open ? body : null}
    </Dialog>
  );
}
