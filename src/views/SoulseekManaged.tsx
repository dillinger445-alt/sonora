/**
 * slskd set up and run by SONORA (macOS): the one-button setup with the
 * screen that lists what it will do, its progress, and slskd's health once
 * it runs (with a fix for each problem SONORA can name).
 */

import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { LuCircle, LuCircleAlert, LuCircleCheck, LuCopy, LuExternalLink, LuFileText, LuFolderOpen, LuLoaderCircle, LuRefreshCw, LuSearch, LuWrench } from 'react-icons/lu';
import type { ManagedSlskd, SetupPlan, SetupRun, SetupStepId, SlskdProcess } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { ProgressBar, Switch } from '../components/Controls.tsx';
import { Dialog, useConfirm } from '../components/Dialog.tsx';
import { copyText } from '../lib/clipboard.ts';
import { formatBytes } from '../lib/format.ts';
import { navigate } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import type { SoulseekState } from './Soulseek.tsx';
import { Group, Row } from './Settings.tsx';

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

const STEP_LABEL: Record<SetupStepId, string> = {
  download: 'Downloading slskd',
  verify: 'Checking the download',
  install: 'Installing',
  configure: 'Writing settings',
  start: 'Starting slskd',
  signin: 'Signing in to Soulseek',
};

/** A path with the home folder written as ~ (shorter, and the same on every Mac). */
function tilde(path: string, home: string | null): string {
  return home && path.startsWith(`${home}/`) ? `~/${path.slice(home.length + 1)}` : path;
}

function insideProtected(path: string, plan: SetupPlan): boolean {
  return plan.protected.some((p) => path === p || path.startsWith(`${p}/`));
}

/** The first thing on the Setup page while slskd is not set up yet. */
export function SetupCard({ onStart, onManual }: { onStart: () => void; onManual: () => void }) {
  return (
    <section className="sk-oneclick" aria-label="Set up Soulseek">
      <div>
        <h2 className="sk-oneclick-title">Set up Soulseek for me</h2>
        <p className="sk-oneclick-lead">
          SONORA downloads slskd, signs it in with your Soulseek account and keeps it running in the background. You type your password and confirm two folders; SONORA does
          the rest and shows each step.
        </p>
      </div>
      <div className="sk-oneclick-actions">
        <Button variant="primary" onClick={onStart}>
          Set up Soulseek…
        </Button>
        <button type="button" className="link sk-manual-link" onClick={onManual}>
          I’ll set up slskd myself
        </button>
      </div>
    </section>
  );
}

/** The one permission screen: what SONORA will do, and the few answers it needs. */
export function SetupDialog({ open, onClose, onStarted, update }: { open: boolean; onClose: () => void; onStarted: () => void; update?: boolean }) {
  const { client, bridge } = useCore();
  const toast = useToast();
  const id = useId();
  const [plan, setPlan] = useState<SetupPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [downloads, setDownloads] = useState('');
  const [sharing, setSharing] = useState(false);
  const [share, setShare] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The first look after opening fills the form from what was found; looks
  // after an action (Stop it, Quit the Soulseek app) only refresh the plan,
  // so a share the person turned off stays off (B04).
  const seeded = useRef(false);
  const load = () => {
    client.slskdSetupPlan().then(
      (p) => {
        setPlan(p);
        setPlanError(null);
        setUsername((u) => u || (p.found?.username ?? ''));
        setDownloads((d) => d || p.downloads);
        if (p.share && !seeded.current) {
          setShare((s) => s || (p.share ?? ''));
          setSharing(true);
        }
        seeded.current = true;
      },
      (e: unknown) => setPlanError(message(e)),
    );
  };

  useEffect(() => {
    if (!open) return;
    setPlan(null);
    setError(null);
    setPassword('');
    seeded.current = false;
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const keepsPassword = !!plan?.found?.hasPassword;
  const ready = !!plan && username.trim() !== '' && downloads.trim() !== '' && (password !== '' || keepsPassword) && (!sharing || share.trim() !== '') && plan.others.length === 0;

  const choose = async (title: string, set: (p: string) => void) => {
    try {
      const p = await bridge.pickFolder(title);
      if (p) set(p);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not open the folder picker', body: message(e) });
    }
  };

  const act = async (what: string, f: () => Promise<unknown>) => {
    setBusy(what);
    try {
      await f();
      load();
    } catch (e) {
      toast({ tone: 'error', title: 'That did not work', body: message(e) });
    } finally {
      setBusy(null);
    }
  };

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!plan || !ready) return;
    setBusy('start');
    setError(null);
    try {
      await client.slskdSetupStart({
        username: username.trim(),
        password: password || undefined,
        downloads: downloads.trim(),
        share: sharing ? share.trim() : null,
        adopt: !password && plan.found ? plan.found.path : null,
      });
      onStarted();
      onClose();
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(null);
    }
  };

  const home = plan?.home || null;
  const d = plan?.download;
  const macOsAsks = plan ? [downloads, sharing ? share : ''].filter((p) => p && insideProtected(p, plan)) : [];

  return (
    <Dialog
      open={open}
      title={update ? `Update slskd to ${plan?.version ?? ''}` : 'Set up Soulseek'}
      onClose={onClose}
      width={600}
      initialFocus={`#${CSS.escape(`${id}-user`)}`}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy === 'start'} disabled={!ready} onClick={() => void submit()}>
            Allow and set up
          </Button>
        </>
      }
    >
      {planError ? (
        <p className="t-amber" role="alert">
          {planError}
        </p>
      ) : !plan ? (
        <p className="faint">Looking at this Mac…</p>
      ) : (
        <form className="sk-setup-form" onSubmit={(e) => void submit(e)}>
          {plan.others.map((p) => (
            <div key={p.pid} className="sk-warn" role="alert">
              <LuCircleAlert aria-hidden />
              <p>
                Another slskd is running ({tilde(p.path, home)}). Only one slskd can run at a time, so it has to stop first.{' '}
                <Button size="sm" busy={busy === `stop-${p.pid}`} onClick={() => void act(`stop-${p.pid}`, () => client.slskdStopOther(p.pid))}>
                  Stop it
                </Button>
              </p>
            </div>
          ))}
          {plan.soulseekQt ? (
            <div className="sk-warn" role="alert">
              <LuCircleAlert aria-hidden />
              <p>
                The Soulseek app is open. One account can be signed in in one place at a time, so quit it before slskd signs in.{' '}
                <Button size="sm" busy={busy === 'quit'} onClick={() => void act('quit', () => client.slskdQuitSoulseekQt())}>
                  Quit the Soulseek app
                </Button>
              </p>
            </div>
          ) : null}

          <div className="sk-fields">
            <label className="field" htmlFor={`${id}-user`}>
              <span className="field-label">Soulseek user name</span>
              <input id={`${id}-user`} className="text-input" autoComplete="off" spellCheck={false} value={username} onChange={(e) => setUsername(e.target.value)} />
            </label>
            <label className="field" htmlFor={`${id}-pw`}>
              <span className="field-label">Password</span>
              <input
                id={`${id}-pw`}
                className="text-input"
                type="password"
                autoComplete="off"
                value={password}
                placeholder={keepsPassword ? 'Leave empty to keep the one you use now' : undefined}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
          </div>
          <p className="field-hint faint">
            The account you use in the Soulseek app. It must not be signed in anywhere else while slskd runs.
            {plan.found ? ` Found in ${tilde(plan.found.path, home)}.` : ''}
          </p>

          <div className="field">
            <span className="field-label" id={`${id}-dl-label`}>
              Folder for finished downloads
            </span>
            <div className="sk-path-row">
              <input className="text-input selectable" aria-labelledby={`${id}-dl-label`} spellCheck={false} value={downloads} onChange={(e) => setDownloads(e.target.value)} />
              {bridge.native ? (
                <Button size="sm" icon={LuFolderOpen} onClick={() => void choose('Choose the folder for finished downloads', setDownloads)}>
                  Choose…
                </Button>
              ) : null}
            </div>
            <p className="field-hint faint">SONORA adds each finished file to your library from here and checks how it sounds.</p>
          </div>

          <div className="sk-share">
            <Switch
              checked={sharing}
              onChange={setSharing}
              label="Share a folder with other Soulseek users"
              description="Off unless you turn it on. Many people on Soulseek only upload to people who share."
            />
            {sharing ? (
              <div className="sk-path-row">
                <input className="text-input selectable" aria-label="Folder to share" spellCheck={false} placeholder="/Users/you/Music/Shared" value={share} onChange={(e) => setShare(e.target.value)} />
                {bridge.native ? (
                  <Button size="sm" icon={LuFolderOpen} onClick={() => void choose('Choose the folder to share', setShare)}>
                    Choose…
                  </Button>
                ) : null}
              </div>
            ) : null}
          </div>

          <h3 className="sk-will-title">SONORA will</h3>
          <ol className="sk-will">
            <li>
              Download slskd {plan.version}
              {d ? ` for ${d.runtime === 'osx-arm64' ? 'Apple silicon' : d.runtime === 'osx-x64' ? 'Intel' : d.runtime} Macs (${formatBytes(d.size)})` : ''} from its official
              release on github.com/slskd/slskd.
            </li>
            <li>Check it against the checksum built into SONORA, and stop if it differs.</li>
            <li>
              Unpack it in <span className="sk-code">{tilde(plan.folder, home)}</span>.
            </li>
            <li>
              Write slskd’s settings: your account, the download folder, a web page only this Mac can open, and a new key for SONORA, kept in your keychain.{' '}
              {sharing ? 'It shares the folder you chose.' : 'It shares nothing.'} Your password goes only into slskd’s settings file, which only your account can read.
            </li>
            <li>
              Add a launch agent{plan.agent ? <> ({tilde(plan.agent, home)})</> : null} so slskd starts when you log in and again if it stops. macOS shows a notice that slskd
              can run in the background.
            </li>
            <li>Start slskd and check that it signs in to Soulseek.</li>
          </ol>
          {macOsAsks.length > 0 ? <p className="sk-note">macOS asks once whether slskd may use {macOsAsks.map((p) => tilde(p, home)).join(' and ')}. Choose Allow.</p> : null}
          {plan.found && plan.found.path.includes('/Desktop/') ? <p className="sk-note faint">Your slskd folder on the Desktop stays as it is. Once this works you can delete it.</p> : null}
          {error ? (
            <p className="field-error" role="alert">
              {error}
            </p>
          ) : null}
          <button type="submit" hidden aria-hidden tabIndex={-1} />
        </form>
      )}
    </Dialog>
  );
}

function StepIcon({ state }: { state: SetupRun['steps'][number]['state'] }) {
  if (state === 'done') return <LuCircleCheck className="t-lime" aria-hidden />;
  if (state === 'running') return <LuLoaderCircle className="spin" aria-hidden />;
  if (state === 'failed') return <LuCircleAlert className="t-amber" aria-hidden />;
  return <LuCircle className="faint" aria-hidden />;
}

/** The setup's checklist, its problem and the way past it. */
/** System Settings → Privacy & Security, where macOS lets a blocked program run. */
const PRIVACY_SECURITY = 'x-apple.systempreferences:com.apple.preference.security?General';

export function SetupProgress({ run, others, onChanged }: { run: SetupRun; others: SlskdProcess[]; onChanged: () => void }) {
  const { client, bridge } = useCore();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState<string | null>(null);

  /** Whether it worked: a form is cleared only then (B60). */
  const act = async (what: string, f: () => Promise<unknown>): Promise<boolean> => {
    setBusy(what);
    try {
      await f();
      onChanged();
      return true;
    } catch (e) {
      toast({ tone: 'error', title: 'That did not work', body: message(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  const retry = (pw?: string) => act('retry', () => client.slskdSetupRetry(pw));

  const discard = async () => {
    const { confirmed } = await confirm({
      title: 'Remove what the setup made?',
      body: (
        <>
          <p>These files and folders go. Nothing else is touched, and a download folder is removed only while it is empty.</p>
          <ul className="sk-file-list">
            {run.created.map((p) => (
              <li key={p} className="selectable mono-path">
                {p}
              </li>
            ))}
          </ul>
        </>
      ),
      confirmLabel: 'Remove them',
      tone: 'danger',
    });
    if (confirmed) await act('discard', () => client.slskdSetupDiscard());
  };

  const err = run.error;
  const pct = run.total > 0 ? run.downloaded / run.total : 0;
  return (
    <section className="sk-progress-panel" aria-label="Setting up slskd" aria-live="polite">
      <ol className="sk-steps">
        {run.steps.map((s) => (
          <li key={s.id} className={`sk-step is-${s.state}`}>
            <StepIcon state={s.state} />
            <span>{STEP_LABEL[s.id]}</span>
            {s.id === 'download' && s.state === 'running' && run.total > 0 ? (
              <span className="sk-step-meter">
                <ProgressBar value={pct} label="Downloading slskd" size="sm" />
                <span className="num faint">
                  {formatBytes(run.downloaded)} of {formatBytes(run.total)}
                </span>
              </span>
            ) : null}
          </li>
        ))}
      </ol>
      {run.finished && !err ? (
        <div className="sk-done">
          <LuCircleCheck className="t-lime" aria-hidden />
          <p>
            Signed in to Soulseek{run.signedInAs ? ` as ${run.signedInAs}` : ''}. slskd now starts by itself when you log in.
          </p>
          <Button size="sm" variant="primary" icon={LuSearch} onClick={() => navigate({ view: 'soulseek', tab: 'search' }, true)}>
            Try a search
          </Button>
        </div>
      ) : null}
      {err ? (
        <div className="sk-step-error" role="alert">
          <p>
            <strong>{STEP_LABEL[err.step]} did not work.</strong> {err.message}
          </p>
          {err.fix === 'password' ? (
            <form
              className="discogs-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (password) void retry(password).then((ok) => ok && setPassword(''));
              }}
            >
              <input className="text-input" type="password" autoComplete="off" aria-label="Soulseek password" placeholder="Soulseek password" value={password} onChange={(e) => setPassword(e.target.value)} />
              <Button size="sm" type="submit" variant="primary" busy={busy === 'retry'} disabled={!password}>
                Try again with this password
              </Button>
            </form>
          ) : null}
          {err.fix === 'stop_other'
            ? others.map((p) => (
                <p key={p.pid} className="sk-inline-act">
                  <span className="selectable mono-path">{p.path}</span>
                  <Button size="sm" busy={busy === `stop-${p.pid}`} onClick={() => void act(`stop-${p.pid}`, () => client.slskdStopOther(p.pid))}>
                    Stop it
                  </Button>
                </p>
              ))
            : null}
          {err.fix === 'quit_soulseek_qt' ? (
            <p className="sk-inline-act">
              <Button size="sm" busy={busy === 'quit'} onClick={() => void act('quit', () => client.slskdQuitSoulseekQt())}>
                Quit the Soulseek app
              </Button>
            </p>
          ) : null}
          {err.fix === 'gatekeeper' ? (
            <p className="sk-inline-act">
              <Button
                size="sm"
                icon={LuExternalLink}
                onClick={() => void bridge.openExternal(PRIVACY_SECURITY).catch((e: unknown) => toast({ tone: 'error', title: 'Could not open System Settings', body: message(e) }))}
              >
                Open Privacy & Security
              </Button>
            </p>
          ) : null}
          {run.created.length > 0 ? (
            <details className="sk-made">
              <summary>What the setup made so far ({run.created.length}): it stays until you remove it</summary>
              <ul className="sk-file-list">
                {run.created.map((p) => (
                  <li key={p} className="selectable mono-path">
                    {p}
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          <div className="sk-key-actions">
            {err.fix !== 'password' || run.passwordKept ? (
              <Button size="sm" variant={err.fix === 'password' ? 'ghost' : 'primary'} icon={LuRefreshCw} busy={busy === 'retry'} onClick={() => void retry()}>
                Try again
              </Button>
            ) : null}
            {run.created.length > 0 ? (
              <Button size="sm" variant="ghost" busy={busy === 'discard'} onClick={() => void discard()}>
                Remove what the setup made
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
      {run.active ? (
        <div className="sk-key-actions">
          <Button size="sm" variant="ghost" onClick={() => void act('cancel', () => client.slskdSetupCancel())}>
            Stop the setup
          </Button>
        </div>
      ) : null}
      {dialogs}
    </section>
  );
}

function Problem({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="sk-warn" role="alert">
      <LuCircleAlert aria-hidden />
      <p>
        {children} {action}
      </p>
    </div>
  );
}

/** slskd as SONORA runs it: how it is doing, and what can be done about it. */
export function ManagedPanel({ m, state, onUpdate }: { m: ManagedSlskd; state: SoulseekState; onUpdate: () => void }) {
  const { client, bridge, settings } = useCore();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const [busy, setBusy] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  const [password, setPassword] = useState('');
  const c = state.connection;
  const signedIn = c?.ok === true && c.info.loggedIn;

  /** Whether it worked: a form is closed only then (B60). */
  const act = async (what: string, f: () => Promise<unknown>, done?: string): Promise<boolean> => {
    setBusy(what);
    try {
      await f();
      state.managed.reload();
      void state.test();
      if (done) toast({ tone: 'success', title: done });
      return true;
    } catch (e) {
      toast({ tone: 'error', title: 'That did not work', body: message(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  const copyWebPassword = async () => {
    const password = client.slskdWebPassword().then(({ password: pw }) => {
      if (!pw) throw new Error('slskd’s settings have no web password.');
      return pw;
    });
    // Handed to the clipboard while the click is handled (see copyText).
    const copied = copyText(password);
    void copied.catch(() => undefined);
    try {
      await password;
      await copied;
      toast({ tone: 'success', title: 'Copied slskd’s web password', body: 'Its user name is slskd.' });
    } catch (e) {
      toast({ tone: 'error', title: 'Could not copy the password', body: message(e) });
    }
  };

  const remove = async () => {
    const { confirmed } = await confirm({
      title: 'Turn slskd off?',
      body: (
        <p>
          slskd stops and no longer starts when you log in or when SONORA opens: SONORA removes its launch agent. slskd’s folder, its settings and your downloads stay
          where they are, and Start brings it back.
        </p>
      ),
      confirmLabel: 'Turn slskd off',
      tone: 'danger',
    });
    if (!confirmed) return;
    setBusy('remove');
    try {
      const r = await client.slskdRemoveAgent();
      state.managed.reload();
      void state.test();
      toast({ tone: 'success', title: 'slskd is off', body: `Its folder, settings and log stay in ${r.kept}. Your downloads stay where they are.` });
    } catch (e) {
      toast({ tone: 'error', title: 'That did not work', body: message(e) });
    } finally {
      setBusy(null);
    }
  };

  const status = m.others.length
    ? 'Not running: another slskd is in the way.'
    : !m.agent.running
      ? !m.agent.installed && !m.agent.loaded
        ? 'Off: its launch agent was removed.'
        : m.stopped
          ? 'Stopped. It stays stopped, also when you log in, until you choose Start.'
          : 'Stopped.'
      : signedIn
        ? `Running${m.background ? ' in the background' : ' while SONORA is open'}, signed in to Soulseek as ${c.info.username ?? m.username ?? ''}.`
        : c && !c.ok
          ? 'Starting, or not answering yet.'
          : 'Running, not signed in to Soulseek.';

  return (
    <Group title="slskd" description={`SONORA set up slskd ${m.installed ?? ''} and keeps it running. Its settings are in ${m.config ?? 'its folder'}.`}>
      <Row label={signedIn ? 'Signed in' : m.agent.running ? 'Running' : 'Not running'} description={status}>
        {m.agent.running ? (
          <>
            <Button size="sm" icon={LuRefreshCw} busy={busy === 'restart'} onClick={() => void act('restart', () => client.slskdService('restart'))}>
              Restart
            </Button>
            <Button size="sm" variant="ghost" busy={busy === 'stop'} onClick={() => void act('stop', () => client.slskdService('stop'))}>
              Stop
            </Button>
          </>
        ) : (
          <Button size="sm" variant="primary" busy={busy === 'start'} disabled={m.others.length > 0} onClick={() => void act('start', () => client.slskdService('start'))}>
            Start
          </Button>
        )}
      </Row>

      {m.others.map((p) => (
        <Problem
          key={p.pid}
          action={
            <Button size="sm" busy={busy === `other-${p.pid}`} onClick={() => void act(`other-${p.pid}`, () => client.slskdStopOther(p.pid), 'Stopped the other slskd')}>
              Stop it
            </Button>
          }
        >
          Another slskd is running (<span className="selectable mono-path">{p.path}</span>), so SONORA’s cannot. It was probably started by hand.
        </Problem>
      ))}
      {m.agent.running && !signedIn && m.reason?.code === 'signed_in_elsewhere' ? (
        <Problem
          action={
            m.soulseekQt ? (
              <Button size="sm" busy={busy === 'quit'} onClick={() => void act('quit', () => client.slskdQuitSoulseekQt().then(() => client.slskdReconnect()))}>
                Quit the Soulseek app
              </Button>
            ) : (
              <Button size="sm" busy={busy === 'reconnect'} onClick={() => void act('reconnect', () => client.slskdReconnect())}>
                Sign in again
              </Button>
            )
          }
        >
          {m.reason.message}
        </Problem>
      ) : null}
      {m.agent.running && !signedIn && (m.reason?.code === 'wrong_password' || m.reason?.code === 'no_account') ? (
        <Problem
          action={
            !changing ? (
              <Button size="sm" onClick={() => setChanging(true)}>
                Change password
              </Button>
            ) : null
          }
        >
          {m.reason.message}
        </Problem>
      ) : null}
      {m.agent.running && !signedIn && m.reason?.code === 'network' ? (
        <Problem
          action={
            <Button size="sm" busy={busy === 'reconnect'} onClick={() => void act('reconnect', () => client.slskdReconnect())}>
              Try now
            </Button>
          }
        >
          {m.reason.message} slskd keeps trying by itself.
        </Problem>
      ) : null}
      {m.keyProblem === 'missing' || (m.keyProblem === 'different' && c && !c.ok && c.code === 'slskd_unauthorized') ? (
        <Problem
          action={
            <Button size="sm" icon={LuWrench} busy={busy === 'repair'} onClick={() => void act('repair', () => client.slskdRepair(), 'Repaired: slskd has SONORA’s key again')}>
              Repair
            </Button>
          }
        >
          slskd’s settings {m.keyProblem === 'missing' ? 'have no key for SONORA' : 'hold a different key than SONORA’s'}. Repair puts SONORA’s key back (after a backup) and
          starts slskd again.
        </Problem>
      ) : null}
      {changing ? (
        <Row label="New Soulseek password" description="It goes into slskd’s settings (after a backup), and slskd starts again with it.">
          <form
            className="discogs-form"
            onSubmit={(e) => {
              e.preventDefault();
              if (!password) return;
              void act('password', () => client.slskdPassword(password), 'Password changed').then((ok) => {
                if (!ok) return;
                setPassword('');
                setChanging(false);
              });
            }}
          >
            <input className="text-input" type="password" autoComplete="off" aria-label="New Soulseek password" value={password} onChange={(e) => setPassword(e.target.value)} />
            <Button size="sm" type="submit" variant="primary" busy={busy === 'password'} disabled={!password}>
              Save
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setChanging(false)}>
              Cancel
            </Button>
          </form>
        </Row>
      ) : null}
      {m.updateAvailable ? (
        <Row label={`slskd ${m.version} is ready to install`} description={`SONORA runs ${m.installed} now. Updating keeps your settings and downloads.`}>
          <Button size="sm" variant="primary" onClick={onUpdate}>
            Update slskd…
          </Button>
        </Row>
      ) : null}

      <Switch
        checked={m.background}
        onChange={(v) => void act('background', () => client.slskdBackground(v))}
        label="Run slskd in the background"
        description={
          m.background
            ? 'slskd starts when you log in and again if it stops, whether SONORA is open or not. Downloads go on after you quit SONORA.'
            : 'slskd runs only while SONORA is open: it starts with SONORA and stops when you quit.'
        }
      />
      <Row label="slskd’s files" description="Its log says what it did and why something failed. Its web page has sharing, chat and every other slskd setting.">
        {m.log ? (
          <Button size="sm" icon={LuFileText} onClick={() => void bridge.reveal(m.log ?? '').catch((e: unknown) => toast({ tone: 'error', title: 'Could not show the log', body: message(e) }))}>
            Show log
          </Button>
        ) : null}
        <Button size="sm" icon={LuExternalLink} onClick={() => void bridge.openExternal(settings?.slskdUrl ?? 'http://127.0.0.1:5030').catch(() => undefined)}>
          Open web page
        </Button>
        <Button size="sm" variant="ghost" icon={LuCopy} onClick={() => void copyWebPassword()}>
          Copy web password
        </Button>
      </Row>
      <Row label="Turn slskd off" description="Stops slskd and removes its launch agent. Its folder, settings and your downloads stay.">
        <Button size="sm" variant="ghost" className="is-danger-text" busy={busy === 'remove'} disabled={!m.agent.installed} onClick={() => void remove()}>
          Turn off
        </Button>
      </Row>
      {dialogs}
    </Group>
  );
}
