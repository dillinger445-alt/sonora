/**
 * What a new version brings, and installing it: download, signature check,
 * replacing this version, restart. The library and settings are kept.
 */

import { LuArrowRight, LuCircleAlert, LuExternalLink, LuTriangleAlert } from 'react-icons/lu';
import { Button } from '../components/Button.tsx';
import { ProgressBar } from '../components/Controls.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { ReleaseNotes } from '../components/ReleaseNotes.tsx';
import { formatBytes } from '../lib/format.ts';
import { useCore, useScanBusy } from '../state/core.tsx';
import { useJobs } from '../state/jobs.tsx';
import { useUpdates } from '../state/updates.tsx';

export const RELEASES_URL = 'https://github.com/dillinger445-alt/sonora/releases';

const dayFmt = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });

/** Work that installing would stop, as a phrase ("the export"), or null. */
function useRunningWork(): string | null {
  const scanBusy = useScanBusy();
  const jobs = useJobs();
  const work = [scanBusy ? 'the analysis' : null, jobs.exportRunning ? 'the export' : null, jobs.discogsRunning ? 'the Discogs lookup' : null].filter(Boolean);
  if (!work.length) return null;
  return work.length === 1 ? (work[0] ?? null) : `${work.slice(0, -1).join(', ')} and ${work[work.length - 1] ?? ''}`;
}

export function UpdateDialog() {
  const u = useUpdates();
  const { bridge, info: appInfo } = useCore();
  const work = useRunningWork();
  const info = u.info;
  const open = u.dialogOpen && (info != null || u.status === 'error');
  const installing = u.status === 'installing';
  const failed = u.status === 'error';

  const notesLink = info ? `${RELEASES_URL}/tag/v${info.version}` : RELEASES_URL;
  const openNotes = () => void bridge.openExternal(notesLink).catch(() => undefined);

  let body;
  let footer;
  if (installing) {
    const s = u.step;
    const fraction = s?.step === 'downloading' ? (s.total ? s.downloaded / s.total : 0) : s ? 1 : 0;
    body = (
      <div className="upd-run" aria-live="polite">
        <p>
          {s?.step === 'downloading'
            ? s.total
              ? `Downloading ${formatBytes(s.downloaded)} of ${formatBytes(s.total)}`
              : `Downloading ${formatBytes(s.downloaded)}`
            : s?.step === 'installing'
              ? 'Checking the signature and installing…'
              : s?.step === 'restarting'
                ? 'Restarting into the new version…'
                : 'Contacting GitHub…'}
        </p>
        <ProgressBar value={fraction} label="Update progress" />
        <p className="faint">SONORA closes and opens again by itself once the new version is in place.</p>
      </div>
    );
    footer = <Button onClick={u.closeDialog}>Hide</Button>;
  } else if (failed) {
    body = (
      <div className="upd-run">
        <p className="upd-error">
          <LuCircleAlert aria-hidden /> <span className="selectable">{u.error}</span>
        </p>
        <p className="faint">
          {u.needsRestart
            ? 'Background work was stopped for the installation. Restart SONORA to carry on; your library and settings are unchanged.'
            : 'Nothing was changed: this version keeps running, with your library and settings as they were.'}
        </p>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" onClick={u.closeDialog}>
          Close
        </Button>
        {u.needsRestart ? (
          <Button variant="primary" onClick={u.restart} data-autofocus>
            Restart SONORA
          </Button>
        ) : info ? (
          <Button variant="primary" onClick={() => void u.install()} data-autofocus>
            Try again
          </Button>
        ) : null}
      </>
    );
  } else if (info) {
    const released = info.date ? Date.parse(info.date) : NaN;
    body = (
      <div className="upd">
        <div className="upd-path" aria-label={`From version ${info.currentVersion} to version ${info.version}`}>
          <span className="upd-ver">{info.currentVersion}</span>
          <span className="upd-wire" aria-hidden>
            <LuArrowRight />
          </span>
          <span className="upd-ver is-new">{info.version}</span>
          {Number.isFinite(released) ? <span className="upd-date faint">released {dayFmt.format(released)}</span> : null}
        </div>
        {info.notes?.trim() ? (
          <div className="upd-notes">
            <ReleaseNotes text={info.notes} />
          </div>
        ) : (
          <p className="faint">This release came without notes.</p>
        )}
        {work ? (
          <p className="upd-warn">
            <LuTriangleAlert aria-hidden /> Installing now stops {work}.
          </p>
        ) : null}
        <p className="faint upd-how">
          SONORA downloads the new version from its GitHub releases, checks its signature, replaces this version and restarts. Your library, settings and edits are
          kept.
        </p>
      </div>
    );
    footer = (
      <>
        <Button variant="ghost" icon={LuExternalLink} className="upd-github" onClick={openNotes}>
          On GitHub
        </Button>
        <Button variant="ghost" onClick={u.closeDialog}>
          Later
        </Button>
        <Button variant="primary" onClick={() => void u.install()} data-autofocus>
          Install and restart
        </Button>
      </>
    );
  }

  const title = installing ? 'Updating SONORA' : failed ? 'The update did not finish' : info ? `SONORA ${info.version} is available` : `SONORA ${appInfo?.version ?? ''}`;
  return (
    <Dialog open={open} title={title} onClose={u.closeDialog} width={560} footer={footer}>
      {body}
    </Dialog>
  );
}
