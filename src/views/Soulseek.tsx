/**
 * Soulseek: searching and downloading through slskd, the Soulseek client the
 * person runs with their own account and sharing settings, and the downloads
 * that arrive in its folder. SONORA never connects to Soulseek itself.
 */

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { LuCircleAlert, LuCircleCheck, LuShieldCheck } from 'react-icons/lu';
import type { DownloadList, SlskdInfo, SlskdStatus, SourcesStatus } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { TabList, tabId } from '../components/Controls.tsx';
import { ErrorState, ViewHeader } from '../components/Layout.tsx';
import { formatRoute, navigate, type SoulseekTab } from '../lib/router.ts';
import { useCore, useCoreEvent } from '../state/core.tsx';
import { throttle } from '../state/emitter.ts';
import { useToast } from '../state/toasts.tsx';
import { useResource, type Resource } from '../state/useResource.ts';
import { SoulseekDownloads } from './SoulseekDownloads.tsx';
import { SoulseekSearch } from './SoulseekSearch.tsx';
import { SoulseekSetup } from './SoulseekSetup.tsx';

/** The notice accepted before anything here works (also shown in the setup). */
export const LAWFUL_USE =
  'SONORA is for music you have the right to download and use. If you use it with Soulseek or another network, you are responsible for making sure each download, copy and use is lawful where you live and allowed by that network’s terms. SONORA does not own, host or license music found on other networks, and it never shares your files.';

export const SEARCHES_SEEN = 'Other Soulseek users see what you search for, along with your Soulseek user name. What slskd shares is up to its own settings.';

/** The last check of slskd: what it said about itself, or why it could not be asked. */
export type Connection = { ok: true; info: SlskdInfo; at: number } | { ok: false; code: string; error: string; at: number } | null;

export interface SoulseekState {
  sources: Resource<SourcesStatus>;
  slskd: Resource<SlskdStatus>;
  connection: Connection;
  testing: boolean;
  test: () => Promise<Connection>;
  downloads: Resource<DownloadList> & { set: (d: DownloadList) => void };
}

/**
 * slskd's downloads, asked for again every couple of seconds while some are
 * on their way or the list is in view, less often otherwise, and not while
 * the window is hidden.
 */
function useDownloads(enabled: boolean, inView: boolean): SoulseekState['downloads'] {
  const { client } = useCore();
  const [data, setData] = useState<DownloadList | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);
  const load = useCallback(() => {
    const id = ++seq.current;
    client.slskdDownloads().then(
      (d) => {
        if (id !== seq.current) return;
        setData(d);
        setError(null);
        setLoading(false);
      },
      (e: unknown) => {
        if (id !== seq.current) return;
        setError(e instanceof Error ? e : new Error(String(e)));
        setLoading(false);
      },
    );
  }, [client]);
  const active = (data?.active ?? 0) > 0;
  const failing = error != null;
  useEffect(() => {
    if (!enabled) return;
    load();
    // Less often while slskd does not answer.
    const every = failing ? 30_000 : inView || active ? 2000 : 8000;
    const timer = setInterval(() => {
      if (!document.hidden) load();
    }, every);
    return () => clearInterval(timer);
  }, [enabled, inView, active, failing, load]);
  // A download was added to the library, or its analysis finished.
  const t = useMemo(() => throttle(load, 1500), [load]);
  useEffect(() => () => t.cancel(), [t]);
  useCoreEvent(['sources.changed', 'sources.imported', 'scan.item', 'scan.finished'], () => {
    if (enabled) t.run();
  });
  return { data, error, loading, reload: load, set: setData };
}

function Intro({ onAccept, busy }: { onAccept: () => void; busy: boolean }) {
  return (
    <div className="sk-intro">
      <h2 className="sk-intro-title">Find lossless music on Soulseek</h2>
      <p className="sk-intro-lead">
        SONORA searches Soulseek through slskd, a Soulseek client that runs on your computer with your own account. Choose what to download; SONORA adds each finished file
        to your library and checks how it really sounds, so a lossy file passed off as lossless shows up straight away.
      </p>
      <div className="sk-notice" role="note" aria-label="Before you start">
        <LuShieldCheck className="sk-notice-icon" aria-hidden />
        <div>
          <p>{LAWFUL_USE}</p>
          <p>{SEARCHES_SEEN}</p>
        </div>
      </div>
      <div className="sk-intro-actions">
        <Button variant="primary" busy={busy} onClick={onAccept}>
          Accept and continue
        </Button>
        <span className="faint sk-fine">SONORA is not affiliated with Soulseek, LLC or with slskd.</span>
      </div>
    </div>
  );
}

function ConnectionLine({ c, keySaved, testing }: { c: Connection; keySaved: boolean; testing: boolean }) {
  if (!keySaved) return <span className="faint">Finish the setup to search.</span>;
  if (!c) return <span className="faint">{testing ? 'Checking slskd…' : 'slskd is not checked yet.'}</span>;
  if (!c.ok)
    return (
      <span className="sk-conn t-amber">
        <LuCircleAlert aria-hidden /> {c.error}
      </span>
    );
  const who = c.info.username ? `as ${c.info.username}` : '';
  if (!c.info.loggedIn)
    return (
      <span className="sk-conn t-amber">
        <LuCircleAlert aria-hidden /> slskd {c.info.version} is running but not signed in to Soulseek.
      </span>
    );
  return (
    <span className="sk-conn">
      <LuCircleCheck aria-hidden className="t-lime" /> Signed in to Soulseek {who} through slskd {c.info.version}
    </span>
  );
}

export function SoulseekView({ tab }: { tab?: SoulseekTab }) {
  const { client } = useCore();
  const toast = useToast();
  const idBase = useId();
  const sources = useResource(() => client.sourcesStatus(), [], { events: ['sources.changed', 'sources.imported'] });
  const slskd = useResource(() => client.slskdStatus(), []);
  const [testing, setTesting] = useState(false);
  const [accepting, setAccepting] = useState(false);
  const accepted = sources.data?.accepted ?? false;
  const keySaved = slskd.data?.keySaved ?? false;
  // The page opens on Search once slskd is set up, otherwise on Setup; it
  // stays where it is while the person sets it up.
  const [opening, setOpening] = useState<SoulseekTab | null>(null);
  if (opening == null && slskd.data) setOpening(slskd.data.keySaved ? 'search' : 'setup');
  const current: SoulseekTab = tab ?? opening ?? 'setup';
  const [connection, setConnection] = useState<Connection>(null);
  // Downloads are asked for once slskd has answered.
  const downloads = useDownloads(accepted && keySaved && connection?.ok === true, current === 'downloads');

  const test = useCallback(async (): Promise<Connection> => {
    setTesting(true);
    try {
      const info = await client.slskdTest();
      const c: Connection = { ok: true, info, at: Date.now() };
      setConnection(c);
      return c;
    } catch (e) {
      const c: Connection = { ok: false, code: (e as { code?: string }).code ?? 'failed', error: e instanceof Error ? e.message : String(e), at: Date.now() };
      setConnection(c);
      return c;
    } finally {
      setTesting(false);
    }
  }, [client]);

  // Ask slskd how it is doing once, when the page opens with slskd set up.
  // After that only when asked: a key just made is not in slskd.yml yet.
  const opened = useRef(false);
  useEffect(() => {
    if (opened.current || !sources.data || !slskd.data) return;
    opened.current = true;
    if (sources.data.accepted && slskd.data.keySaved) void test();
  }, [sources.data, slskd.data, test]);
  useEffect(() => {
    if (!keySaved) setConnection(null);
  }, [keySaved]);

  const accept = async () => {
    setAccepting(true);
    try {
      await client.sourcesAccept();
      sources.reload();
      slskd.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not save that', body: e instanceof Error ? e.message : String(e) });
    } finally {
      setAccepting(false);
    }
  };

  const state: SoulseekState = { sources, slskd, connection, testing, test, downloads };
  const waiting = sources.data?.counts.review ?? 0;
  const activeCount = downloads.data?.active ?? 0;

  if (sources.error || slskd.error) {
    return (
      <div className="view view-scroll">
        <ViewHeader title="Soulseek" />
        <ErrorState
          title="Could not load the Soulseek settings"
          error={(sources.error ?? slskd.error) as Error}
          onRetry={() => {
            sources.reload();
            slskd.reload();
          }}
        />
      </div>
    );
  }
  if (!sources.data || !slskd.data) {
    return (
      <div className="view">
        <ViewHeader title="Soulseek" />
      </div>
    );
  }

  return (
    <div className="view view-scroll sk-view">
      <ViewHeader
        title="Soulseek"
        subtitle={accepted ? <ConnectionLine c={connection} keySaved={keySaved} testing={testing} /> : 'Search and download through slskd, the Soulseek client on your computer.'}
      />
      {!accepted ? (
        <Intro onAccept={() => void accept()} busy={accepting} />
      ) : (
        <>
          <TabList
            items={[
              { id: 'search', label: 'Search' },
              { id: 'downloads', label: 'Downloads', count: activeCount + waiting || undefined },
              { id: 'setup', label: 'Setup' },
            ]}
            value={current}
            onChange={(t) => navigate({ view: 'soulseek', tab: t }, true)}
            label="Soulseek"
            idBase={idBase}
            panelId={`${idBase}-panel`}
            variant="underline"
          />
          <div className="sk-panel" role="tabpanel" id={`${idBase}-panel`} aria-labelledby={tabId(idBase, current)}>
            {current === 'search' ? (
              keySaved ? (
                <SoulseekSearch state={state} />
              ) : (
                <p className="faint sk-gap">
                  Searching needs slskd. <a className="link" href={formatRoute({ view: 'soulseek', tab: 'setup' })}>Set it up</a> first.
                </p>
              )
            ) : current === 'downloads' ? (
              <SoulseekDownloads state={state} />
            ) : (
              <SoulseekSetup state={state} />
            )}
          </div>
        </>
      )}
    </div>
  );
}
