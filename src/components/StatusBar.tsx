import { LuShieldCheck, LuTriangleAlert } from 'react-icons/lu';
import { formatCount, formatEta } from '../lib/format.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore, useScan } from '../state/core.tsx';
import { ProgressBar } from './Controls.tsx';

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
        {ffmpeg && !ffmpeg.available ? (
          <a className="sb-item t-amber" href={formatRoute({ view: 'settings' })} data-tip="Without FFmpeg, only WAV, AIFF and FLAC can be analyzed. See Settings.">
            <LuTriangleAlert aria-hidden />
            FFmpeg not found
          </a>
        ) : null}
        <span
          className="sb-item"
          data-tip="Analysis runs on this computer. Audio, results and file paths are never uploaded, and SONORA makes no network requests."
        >
          <LuShieldCheck aria-hidden />
          Local only
        </span>
        {info ? <span className="sb-item faint num">v{info.version}</span> : null}
      </div>
    </footer>
  );
}
