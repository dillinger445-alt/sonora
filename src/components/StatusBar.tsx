import { LuDisc3, LuShieldCheck, LuTriangleAlert, LuUsb, LuX } from 'react-icons/lu';
import { formatCount, formatEta } from '../lib/format.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore, useScan } from '../state/core.tsx';
import { useJobs } from '../state/jobs.tsx';
import { ProgressBar } from './Controls.tsx';

/** Exports and Discogs lookups running in the background. */
function JobItems() {
  const jobs = useJobs();
  const ex = jobs.exportProgress;
  const dg = jobs.discogsProgress;
  return (
    <>
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
        <JobItems />
        {ffmpeg && !ffmpeg.available ? (
          <a className="sb-item t-amber" href={formatRoute({ view: 'settings' })} data-tip="Without FFmpeg, only WAV, AIFF and FLAC can be analyzed. See Settings.">
            <LuTriangleAlert aria-hidden />
            FFmpeg not found
          </a>
        ) : null}
        <span
          className="sb-item"
          data-tip="Analysis and playback run on this computer. Audio, results and file paths are never uploaded. SONORA goes online only for Discogs genre lookups you start, which send artist and title."
        >
          <LuShieldCheck aria-hidden />
          Local only
        </span>
        {info ? <span className="sb-item faint num">v{info.version}</span> : null}
      </div>
    </footer>
  );
}
