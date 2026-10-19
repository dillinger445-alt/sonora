import { useState, type ReactNode } from 'react';
import { LuDatabaseBackup, LuFolderSearch, LuRefreshCw, LuShieldCheck, LuTrash2 } from 'react-icons/lu';
import type { KeyNotation, Settings } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Segmented, Switch } from '../components/Controls.tsx';
import { useConfirm } from '../components/Dialog.tsx';
import { ViewHeader } from '../components/Layout.tsx';
import { formatBytes, formatCount, plural } from '../lib/format.ts';
import { formatKey, KEY_NOTATIONS } from '../lib/keys.ts';
import { formatRoute } from '../lib/router.ts';
import { SCORE_HINT, SCORE_KINDS, SCORE_LABEL } from '../lib/scores.ts';
import { useCore, useScanBusy } from '../state/core.tsx';
import { isAbsolutePath } from '../lib/paths.ts';
import { useToast } from '../state/toasts.tsx';
import { revealLabel } from '../lib/platform.ts';
import { useResource } from '../state/useResource.ts';
import { DiscogsSettings } from './DiscogsSettings.tsx';

/** Tempo ranges DJs commonly use (each spans an octave, so every tempo has one reading in it). */
const BPM_RANGES: [number, number][] = [
  [60, 120],
  [70, 140],
  [70, 180],
  [80, 160],
  [88, 175],
  [98, 195],
];

const FOLDER_GENRES: { value: Settings['genreFromFolders']; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'top', label: 'Top folder' },
  { value: 'parent', label: 'Containing folder' },
];

const A_MINOR = 21;

function Group({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section className="settings-group" aria-label={title}>
      <div className="settings-intro">
        <h2 className="settings-title">{title}</h2>
        {description ? <p className="settings-desc">{description}</p> : null}
      </div>
      <div className="settings-body">{children}</div>
    </section>
  );
}

function Row({ label, description, children }: { label: string; description?: ReactNode; children: ReactNode }) {
  return (
    <div className="setting-row">
      <div className="switch-text">
        <span className="switch-label">{label}</span>
        {description ? <span className="switch-desc">{description}</span> : null}
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

function NumberStepper({ value, min, max, step = 1, onChange, label, unit }: { value: number; min: number; max: number; step?: number; onChange: (v: number) => void; label: string; unit?: string }) {
  const [text, setText] = useState(String(value));
  const [lastValue, setLastValue] = useState(value);
  if (value !== lastValue) {
    setLastValue(value);
    setText(String(value));
  }
  const commit = (raw: string) => {
    const v = Number(raw);
    if (!Number.isFinite(v)) {
      setText(String(value));
      return;
    }
    const clamped = Math.max(min, Math.min(max, Math.round(v / step) * step));
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };
  return (
    <div className="stepper">
      <button type="button" className="stepper-btn" aria-label={`Decrease ${label}`} disabled={value <= min} onClick={() => onChange(Math.max(min, value - step))}>
        −
      </button>
      <input
        className="stepper-input num"
        inputMode="numeric"
        aria-label={label}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit((e.target as HTMLInputElement).value);
          if (e.key === 'ArrowUp') {
            e.preventDefault();
            onChange(Math.min(max, value + step));
          }
          if (e.key === 'ArrowDown') {
            e.preventDefault();
            onChange(Math.max(min, value - step));
          }
        }}
      />
      <button type="button" className="stepper-btn" aria-label={`Increase ${label}`} disabled={value >= max} onClick={() => onChange(Math.min(max, value + step))}>
        +
      </button>
      {unit ? <span className="stepper-unit faint">{unit}</span> : null}
    </div>
  );
}

export function SettingsView() {
  const { settings, saveSettings, info, client, bridge, refreshInfo, overview } = useCore();
  const busy = useScanBusy();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const cache = useResource(() => client.cacheStats(), [], { events: ['scan.finished'] });
  const [backupPath, setBackupPath] = useState('');
  const [checking, setChecking] = useState(false);

  const save = (values: Partial<Settings>) =>
    saveSettings(values).catch((e: unknown) => toast({ tone: 'error', title: 'Could not save the setting', body: e instanceof Error ? e.message : String(e) }));

  const backup = async (path: string) => {
    try {
      const r = await client.backup(path);
      toast({ tone: 'success', title: 'Library backed up', body: r.path, action: { label: revealLabel(bridge.platform), run: () => void bridge.reveal(r.path) } });
    } catch (e) {
      toast({ tone: 'error', title: 'Backup failed', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const startBackup = async () => {
    if (bridge.native) {
      const d = new Date();
      const p = (n: number) => String(n).padStart(2, '0');
      const path = await bridge.pickSavePath(`sonora-library-${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}.db`);
      if (path) await backup(path);
    } else if (isAbsolutePath(backupPath.trim())) {
      await backup(backupPath.trim());
    } else {
      toast({ tone: 'warning', title: 'Enter a full path for the backup file' });
    }
  };

  const clearVisuals = async () => {
    const { confirmed } = await confirm({
      title: 'Clear stored visualizations?',
      body: (
        <>
          <p>Waveforms, spectrograms and loudness curves are removed from the library database to free space. Scores and findings are kept.</p>
          <p className="faint">Re-analyzing a track brings its visualizations back.</p>
        </>
      ),
      confirmLabel: 'Clear visualizations',
    });
    if (!confirmed) return;
    try {
      const r = await client.clearVisuals();
      toast({ tone: 'success', title: `Cleared visualizations of ${plural(r.cleared, 'track')}` });
      cache.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not clear the cache', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const checkTools = async () => {
    setChecking(true);
    try {
      await client.refreshTools();
      await refreshInfo();
    } finally {
      setChecking(false);
    }
  };

  if (!settings || !info) {
    return (
      <div className="view">
        <ViewHeader title="Settings" />
      </div>
    );
  }

  return (
    <div className="view view-scroll">
      <ViewHeader title="Settings" subtitle="Changes are saved immediately." />
      <div className="settings">
        <Group title="Analysis" description="How the background analysis uses this computer.">
          <Row
            label="Parallel analyses"
            description={`Files analyzed at the same time. This computer has ${info.cpus} processor cores; more parallel work finishes sooner but uses more memory. Changes apply from the next scan.`}
          >
            <NumberStepper value={settings.workers} min={1} max={16} label="Parallel analyses" onChange={(v) => void save({ workers: v })} />
          </Row>
          <Row label="Longest file analyzed" description="Longer files (DJ mixes, recordings) are not analyzed; they are listed as failed with the reason.">
            <NumberStepper value={settings.maxDurationMin} min={1} max={1440} step={1} label="Longest file in minutes" unit="min" onChange={(v) => void save({ maxDurationMin: v })} />
          </Row>
          <Switch
            label="Re-analyze changed files"
            description="When a file's size or modification time changes, analyze it again on the next scan."
            checked={settings.reanalyzeChanged}
            onChange={(v) => void save({ reanalyzeChanged: v })}
          />
          <Switch
            label="Check library folders at launch"
            description="Look for new, changed, moved and missing files every time SONORA starts."
            checked={settings.rescanOnLaunch === 'changed'}
            onChange={(v) => void save({ rescanOnLaunch: v ? 'changed' : 'off' })}
          />
          <Switch label="Skip hidden files and folders" description="Names starting with a dot, and system folders." checked={settings.skipHidden} onChange={(v) => void save({ skipHidden: v })} />
          <Switch
            label="Follow symbolic links"
            description="Include files reached through links and aliases. Off by default to avoid scanning the same music twice."
            checked={settings.followSymlinks}
            onChange={(v) => void save({ followSymlinks: v })}
          />
        </Group>

        <Group title="Display">
          <Row label="Default score" description="The score highlighted in lists and used for the quality profile.">
            <Segmented
              label="Default score"
              size="sm"
              value={settings.defaultScore}
              onChange={(v) => void save({ defaultScore: v })}
              options={SCORE_KINDS.map((k) => ({ value: k, label: SCORE_LABEL[k], hint: SCORE_HINT[k] }))}
            />
          </Row>
          <Row label="Theme" description="Only the dark theme is available in this version.">
            <Segmented label="Theme" size="sm" value="dark" onChange={() => undefined} options={[{ value: 'dark', label: 'Dark' }]} />
          </Row>
        </Group>

        <Group title="Tempo and key" description="How tempo and key are shown in lists and filters, and written into DJ exports.">
          <Row
            label="Key notation"
            description={`${KEY_NOTATIONS.find((n) => n.id === settings.keyNotation)?.description ?? ''}. A minor is shown as ${formatKey(A_MINOR, settings.keyNotation)}.`}
          >
            <Segmented<KeyNotation>
              label="Key notation"
              size="sm"
              value={settings.keyNotation}
              onChange={(v) => void save({ keyNotation: v })}
              options={KEY_NOTATIONS.map((n) => ({ value: n.id, label: n.label, hint: `${n.description} (A minor: ${formatKey(A_MINOR, n.id)})` }))}
            />
          </Row>
          <Row
            label="BPM range"
            description="Detected tempos are shown within this range, so half-time and double-time readings land where you expect: a drum & bass track reads 174 in 88–175, but 87 in 70–140. Your own edits are kept as typed."
          >
            <Segmented
              label="BPM range"
              size="sm"
              value={`${settings.bpmRange[0]}-${settings.bpmRange[1]}`}
              onChange={(v) => {
                const [lo, hi] = v.split('-').map(Number);
                if (lo != null && hi != null) void save({ bpmRange: [lo, hi] });
              }}
              options={[
                ...BPM_RANGES.map(([lo, hi]) => ({ value: `${lo}-${hi}`, label: `${lo}–${hi}` })),
                ...(BPM_RANGES.some(([lo, hi]) => lo === settings.bpmRange[0] && hi === settings.bpmRange[1])
                  ? []
                  : [{ value: `${settings.bpmRange[0]}-${settings.bpmRange[1]}`, label: `${settings.bpmRange[0]}–${settings.bpmRange[1]}` }]),
              ]}
            />
          </Row>
        </Group>

        <Group title="Genres" description="Genres come from the files' tags first. Tracks without one can take it from their folder, from Discogs, or from what you type.">
          <Row
            label="Folder names as genres"
            description="For tracks without a genre tag. Top folder: in Music/House/Artist/track.mp3 the genre is House. Containing folder: the folder the file is in."
          >
            <Segmented label="Folder names as genres" size="sm" value={settings.genreFromFolders} onChange={(v) => void save({ genreFromFolders: v })} options={FOLDER_GENRES} />
          </Row>
          <DiscogsSettings />
        </Group>

        <Group title="Library" description={<>Folders are managed on the <a className="link" href={formatRoute({ view: 'folders' })}>Folders</a> page.</>}>
          <Row label="Library database" description={<span className="selectable mono-path">{info.dbPath}</span>}>
            <Button size="sm" icon={LuFolderSearch} onClick={() => void bridge.reveal(info.dbPath).catch((e: unknown) => toast({ tone: 'error', title: 'Could not open the folder', body: String(e) }))}>
              {revealLabel(bridge.platform)}
            </Button>
          </Row>
          <Row
            label="Back up the library"
            description="Writes a consistent copy of the database (tracks, results, history, settings). To restore, quit SONORA and replace the database file with the copy."
          >
            {bridge.native ? null : (
              <input
                className="text-input selectable"
                placeholder="/full/path/sonora-backup.db"
                aria-label="Backup file path"
                value={backupPath}
                onChange={(e) => setBackupPath(e.target.value)}
              />
            )}
            <Button size="sm" icon={LuDatabaseBackup} onClick={() => void startBackup()}>
              Back up…
            </Button>
          </Row>
          <Row
            label="Stored data"
            description={
              cache.data
                ? `${formatBytes(cache.data.dbBytes)} in total, of which ${formatBytes(cache.data.visualsBytes)} are visualizations for ${formatCount(cache.data.analyses)} analyzed tracks.`
                : 'Measuring…'
            }
          >
            <Button size="sm" icon={LuTrash2} variant="ghost" className="is-danger-text" disabled={busy || !cache.data?.visualsBytes} onClick={() => void clearVisuals()}>
              Clear visualizations
            </Button>
          </Row>
          <Switch
            label="Keep visualizations"
            description="Store waveform, spectrogram and loudness data with each analysis (roughly 100 kB per track). Turn off to save space; scores and findings are always kept."
            checked={settings.keepVisuals}
            onChange={(v) => void save({ keepVisuals: v })}
          />
        </Group>

        <Group title="Decoding" description="WAV, AIFF and FLAC are read natively. Other formats are decoded with FFmpeg.">
          <Row
            label="FFmpeg"
            description={
              info.ffmpeg.available ? (
                <span className="selectable">
                  {info.ffmpeg.version ?? 'Found'}
                  <br />
                  <span className="mono-path">{info.ffmpeg.path}</span>
                </span>
              ) : (
                <span className="t-amber">
                  Not found. MP3, AAC/M4A, ALAC, Ogg Vorbis and Opus files cannot be analyzed until FFmpeg is available. The desktop app ships with its own copy;
                  in development, install FFmpeg on your PATH.
                </span>
              )
            }
          >
            <Button size="sm" icon={LuRefreshCw} busy={checking} onClick={() => void checkTools()}>
              Check again
            </Button>
          </Row>
        </Group>

        <Group title="Privacy">
          <div className="privacy">
            <LuShieldCheck className="privacy-icon" aria-hidden />
            <div>
              <p>
                SONORA works entirely on this computer: no uploads, no analytics, no telemetry, no update checks. The only network requests are Discogs genre lookups you start
                yourself, which send the artist and title of the tracks you chose.
              </p>
              <p className="faint">
                The library database stores file paths, tags, measurements, scores, visualizations and your tempo, key and genre edits for the{' '}
                {overview ? formatCount(overview.tracks.total) : ''} tracks you added. Removing a track removes its data; your audio files are never modified, moved or
                deleted. Exports write new files and never replace existing ones.
              </p>
              <p className="faint">
                Playback reads your files where they are. Formats this window cannot play directly are decoded once into SONORA's cache folder, which keeps the six most
                recent.
              </p>
            </div>
          </div>
        </Group>

        <Group title="About">
          <dl className="about">
            <div>
              <dt>Version</dt>
              <dd className="num">{info.version}</dd>
            </div>
            <div>
              <dt>Scoring model</dt>
              <dd className="num">{info.modelVersion}</dd>
            </div>
            <div>
              <dt>Analysis engine</dt>
              <dd className="num">
                {info.engineVersion} <span className="faint">(measurements v{info.measurementSchema})</span>
              </dd>
            </div>
            <div>
              <dt>SQLite</dt>
              <dd className="num">{info.sqlite}</dd>
            </div>
            <div>
              <dt>System</dt>
              <dd className="num">
                {info.platform} {info.arch}, {info.cpus} cores
              </dd>
            </div>
            <div>
              <dt>Data folder</dt>
              <dd className="selectable mono-path">{info.dataDir}</dd>
            </div>
          </dl>
        </Group>
      </div>
      {dialogs}
    </div>
  );
}
