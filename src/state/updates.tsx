/**
 * In-app updates: looks for a newer SONORA release on GitHub (on launch and
 * every six hours while the setting is on, or when asked), and installs it
 * when the listener chooses to. Installing replaces the app and restarts it;
 * the library database and the window's preferences live elsewhere and are
 * kept.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { CoreError, type UpdateInfo, type UpdateStep } from '../api/bridge.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { useCore } from './core.tsx';
import { useToast } from './toasts.tsx';

/** How often a running app looks again. */
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
/** The first automatic check waits until the window has settled. */
const FIRST_CHECK_MS = 10_000;

export type UpdateStatus = 'idle' | 'checking' | 'current' | 'available' | 'installing' | 'error';

export interface Updates {
  /** This build can update itself (the desktop app on macOS and Windows). */
  supported: boolean;
  status: UpdateStatus;
  /** The newer release, once found. */
  info: UpdateInfo | null;
  /** While installing. */
  step: UpdateStep | null;
  error: string | null;
  /** The installation failed after background work was stopped: restarting is the way on. */
  needsRestart: boolean;
  /** When GitHub last answered. */
  lastChecked: number | null;
  /** Looks now; `manual` reports the outcome even when there is nothing new. */
  check: (manual?: boolean) => Promise<void>;
  install: () => Promise<void>;
  restart: () => void;
  dialogOpen: boolean;
  openDialog: () => void;
  closeDialog: () => void;
}

const UpdatesContext = createContext<Updates | null>(null);

export function useUpdates(): Updates {
  const v = useContext(UpdatesContext);
  if (!v) throw new Error('useUpdates outside UpdatesProvider');
  return v;
}

const isString = (v: unknown): v is string => typeof v === 'string';

export function UpdatesProvider({ children }: { children: ReactNode }) {
  const { bridge, settings, info: appInfo } = useCore();
  const toast = useToast();
  const updater = bridge.updater;
  const [status, setStatus] = useState<UpdateStatus>('idle');
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [step, setStep] = useState<UpdateStep | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [needsRestart, setNeedsRestart] = useState(false);
  const [lastChecked, setLastChecked] = useState<number | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const busy = useRef(false);

  const check = useCallback(
    async (manual = false) => {
      if (!updater || busy.current) return;
      busy.current = true;
      setStatus((s) => (s === 'installing' ? s : 'checking'));
      setError(null);
      try {
        const found = await updater.check();
        setLastChecked(Date.now());
        setInfo(found);
        setStatus(found ? 'available' : 'current');
        if (found && (manual || readPref('updates.announced', '', isString) !== found.version)) {
          writePref('updates.announced', found.version);
          if (manual) setDialogOpen(true);
          else
            toast({
              tone: 'info',
              title: `SONORA ${found.version} is available`,
              body: 'Install it when it suits you; your library and settings are kept.',
              action: { label: 'See what’s new', run: () => setDialogOpen(true) },
              timeout: 12_000,
            });
        } else if (!found && manual) {
          toast({ tone: 'success', title: 'SONORA is up to date', body: `Version ${appInfo?.version ?? ''} is the newest release.` });
        }
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        // An automatic check that fails (offline, say) stays quiet.
        setStatus('idle');
        if (manual) {
          setError(message);
          toast({ tone: 'error', title: 'Could not check for updates', body: message });
        }
      } finally {
        busy.current = false;
      }
    },
    [updater, toast, appInfo],
  );

  const install = useCallback(async () => {
    if (!updater || busy.current) return;
    busy.current = true;
    setStatus('installing');
    setError(null);
    setNeedsRestart(false);
    setStep(null);
    try {
      await updater.install(setStep);
      // Only reached when the restart did not happen.
      setStatus('error');
      setNeedsRestart(true);
      setError('The new version is installed but SONORA did not restart by itself.');
    } catch (e) {
      const code = e instanceof CoreError ? e.code : '';
      setStatus('error');
      // Past the download, background work has been stopped for the installation.
      setNeedsRestart(code === 'update_install_failed' || code === 'update_restart_failed');
      setError(e instanceof Error ? e.message : String(e));
      setDialogOpen(true);
    } finally {
      setStep(null);
      busy.current = false;
    }
  }, [updater]);

  // Automatic checks, while the setting is on.
  const auto = !!updater && settings?.checkUpdates === true;
  useEffect(() => {
    if (!auto) return;
    const first = setTimeout(() => void check(false), FIRST_CHECK_MS);
    const every = setInterval(() => void check(false), CHECK_EVERY_MS);
    return () => {
      clearTimeout(first);
      clearInterval(every);
    };
  }, [auto, check]);

  // After an update: say so once.
  const version = appInfo?.version;
  useEffect(() => {
    if (!version) return;
    const before = readPref('updates.lastVersion', '', isString);
    writePref('updates.lastVersion', version);
    if (before && before !== version) toast({ tone: 'success', title: `SONORA was updated to ${version}`, body: 'Your library and settings are as you left them.' });
  }, [version, toast]);

  const value = useMemo<Updates>(
    () => ({
      supported: !!updater,
      status,
      info,
      step,
      error,
      lastChecked,
      needsRestart,
      check,
      install,
      restart: () => void updater?.restart().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e))),
      dialogOpen,
      openDialog: () => setDialogOpen(true),
      closeDialog: () => setDialogOpen(false),
    }),
    [updater, status, info, step, error, needsRestart, lastChecked, check, install, dialogOpen],
  );
  return <UpdatesContext.Provider value={value}>{children}</UpdatesContext.Provider>;
}
