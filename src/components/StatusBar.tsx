import { LuCircleArrowUp, LuDisc3, LuListMusic, LuShieldCheck, LuTriangleAlert, LuUsb, LuX } from 'react-icons/lu';
import { formatCount, formatEta, formatPercent } from '../lib/format.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore, useScan } from '../state/core.tsx';
import { useJobs } from '../state/jobs.tsx';
import { useUpdates } from '../state/updates.tsx';
import { ProgressBar } from './Controls.tsx';

/** A newer version, or one being installed. */
function UpdateItem() {
  const u = useUpdates();
  if (!u.supported) return null;
  if (u.status === 'installing') {
    const s = u.step;
    const pct = s?.step === 'downloading' && s.total ? formatPercent(s.downloaded / s.total) : null;
    return (
      <button type="button" className="sb-item sb-job sb-button" onClick={u.openDialog}>
        <LuCircleArrowUp aria-hidden />
        <span>Updating</span>
        {pct ? <span className="num">{pct}</span> : null}
      </button>
    );
  }
  if (u.status === 'error' && u.info) {
    return (
      <button type="button" className="sb-item sb-button t-amber" onClick={u.openDialog}>
        <LuTriangleAlert aria-hidden />
        Update stopped
      </button>
    );
  }
  if (u.status === 'available' && u.info) {
    return (
      <button type="button" className="sb-item sb-button sb-update" onClick={u.openDialog} data-tip={`SONORA ${u.info.version} is available: see what is new`}>
        <LuCircleArrowUp aria-hidden />
        Update to {u.info.version}
      </button>
    );
  }
  return null;
}

/** Exports, Discogs lookups and playlist downloads running in the background. */
function JobItems() {
  const jobs = useJobs();
  const ex = jobs.exportProgress;
  const dg = jobs.discogsProgress;
  const usb = jobs.usbProgress;
  const pl = jobs.playlistRun;
  const plSettled = pl ? pl.counts.done + pl.counts.failed + pl.counts.notFound : 0;
  return (
    <>
      {pl ? (
        <span className="sb-item sb-job" role="status">
          <LuListMusic aria-hidden />
          <a className="sb-link" href={formatRoute({ view: 'soulseek', tab: 'playlist' })}>
            {pl.state === 'paused' ? 'Playlist paused' : 'Playlist'}
          </a>
          <span className="num">
            {formatCount(plSettled)}/{formatCount(pl.counts.included)}
          </span>
          <span className="sb-job-bar">
            <ProgressBar value={pl.counts.included > 0 ? plSettled / pl.counts.included : 0} label="Playlist progress" size="sm" />
          </span>
          <button type="button" className="sb-job-stop" aria-label="Stop the playlist" data-tip="Stop the playlist" onClick={jobs.cancelPlaylist}>
            <LuX aria-hidden />
          </button>
        </span>
      ) : null}
      {jobs.usbRunning ? (
        <span className="sb-item sb-job" role="status">
          <LuUsb aria-hidden />
          <span>Sending to drive</span>
          {usb && usb.total > 0 ? (
            <span className="num">
              {formatCount(Math.min(usb.done + 1, usb.total))}/{formatCount(usb.total)}
            </span>
          ) : null}
          <span className="sb-job-bar">
            <ProgressBar value={usb?.fraction ?? 0} label="Sending progress" size="sm" />
          </span>
          <button type="button" className="sb-job-stop" aria-label="Stop sending" data-tip="Stop sending" onClick={jobs.cancelUsb}>
            <LuX aria-hidden />
          </button>
        </span>
      ) : null}
      {jobs.exportRunning ? (
        <span className="sb-item sb-job" role="status">
          <LuUsb aria-hidden />
          <span>Exporting</span>
          {ex ? (
            <span className="num">
              {formatCount(Math.min(ex.done + 1, ex.total))}/{formatCount(ex.total)}
            </span>
          ) : null}
          <span className="sb-job-bar">
            <ProgressBar value={ex?.fraction ?? 0} label="Export progress" size="sm" />
          </span>
          <button type="button" className="sb-job-stop" aria-label="Stop the export" data-tip="Stop the export" onClick={jobs.cancelExport}>
            <LuX aria-hidden />
          </button>
        </span>
      ) : null}
      {jobs.discogsRunning ? (
        <span className="sb-item sb-job" role="status">
          <LuDisc3 aria-hidden />
          <span>Discogs genres</span>
          {dg ? (
            <span className="num">
              {formatCount(Math.min(dg.done + 1, dg.total))}/{formatCount(dg.total)}
            </span>
          ) : null}
          <button type="button" className="sb-job-stop" aria-label="Stop the Discogs lookup" data-tip="Stop the lookup" onClick={jobs.cancelDiscogs}>
            <LuX aria-hidden />
          </button>
        </span>
      ) : null}
    </>
  );
}

export function StatusBar() {
  const { overview, info } = useCore();
  const { scan } = useScan();
  const state = scan?.state ?? 'idle';
  const done = scan?.done ?? 0;
  const current = scan?.current[0];

  let left;
  if (state === 'indexing') {
    left = (
      <>
        <span className="sb-dot is-indexing" aria-hidden />
        <span className="ellipsis">{scan?.indexing ?? 'Looking for audio files…'}</span>
      </>
    );
  } else if (state === 'running' || state === 'paused' || state === 'cancelling') {
    const label = state === 'paused' ? 'Paused' : state === 'cancelling' ? 'Stopping' : 'Analyzing';
    left = (
      <>
        <span className={`sb-dot is-${state}`} aria-hidden />
        <span className="sb-strong">{label}</span>
        <span className="num">
          {formatCount(done)}/{formatCount(scan?.total ?? 0)}
        </span>
        <span className="sb-progress">
          <ProgressBar value={scan && scan.total > 0 ? done / scan.total : 0} label="Scan progress" size="sm" tone={state === 'paused' ? 'amber' : 'lime'} />
        </span>
        {state === 'running' && current ? <span className="ellipsis faint">{current.fileName}</span> : null}
        {state === 'running' && scan?.etaMs != null ? <span className="faint sb-eta">{formatEta(scan.etaMs)} left</span> : null}
      </>
    );
  } else {
    const t = overview?.tracks;
    left = (
      <>
        <span className="sb-dot" aria-hidden />
        <span className="faint">
          {t
            ? t.total === 0
              ? 'No tracks yet'
              : `${formatCount(t.total)} tracks, ${formatCount(t.analyzed)} analyzed${t.pending ? `, ${formatCount(t.pending)} waiting` : ''}${t.failed ? `, ${formatCount(t.failed)} failed` : ''}`
            : 'Connecting to the analysis core…'}
        </span>
      </>
    );
  }

  const ffmpeg = info?.ffmpeg;
  return (
    <footer className="statusbar" aria-label="Status">
      <a className="sb-left" href={formatRoute({ view: 'queue' })} aria-live="off">
        {left}
      </a>
      <div className="sb-right">
        <UpdateItem />
        <JobItems />
        {ffmpeg && !ffmpeg.available ? (
          <a className="sb-item t-amber" href={formatRoute({ view: 'settings' })} data-tip="Without FFmpeg, only WAV, AIFF and FLAC can be analyzed. See Settings.">
            <LuTriangleAlert aria-hidden />
            FFmpeg not found
          </a>
        ) : null}
        <span
          className="sb-item"
          data-tip="Analysis and playback run on this computer. Audio, results and file paths are never uploaded. SONORA goes online only to ask GitHub for a newer version (can be turned off in Settings) and for Discogs genre lookups you start, which send artist and title. Soulseek searches and downloads go through slskd, the Soulseek client you run."
        >
          <LuShieldCheck aria-hidden />
          Local only
        </span>
        {info ? <span className="sb-item faint num">v{info.version}</span> : null}
      </div>
    </footer>
  );
}
