/**
 * Setting up Soulseek: slskd (the client SONORA searches through), its API
 * key, its address, and the folder where finished downloads arrive.
 */

import { useState } from 'react';
import { LuCopy, LuExternalLink, LuFolderOpen, LuKeyRound, LuRefreshCw, LuShieldCheck } from 'react-icons/lu';
import { Button } from '../components/Button.tsx';
import { Segmented, Switch } from '../components/Controls.tsx';
import { useConfirm } from '../components/Dialog.tsx';
import { copyText } from '../lib/clipboard.ts';
import { formatDate, formatRelative, plural } from '../lib/format.ts';
import { isAbsolutePath } from '../lib/paths.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import { LAWFUL_USE, SEARCHES_SEEN, type SoulseekState } from './Soulseek.tsx';
import { ManagedPanel, SetupCard, SetupDialog, SetupProgress } from './SoulseekManaged.tsx';
import { Group, Row } from './Settings.tsx';

const SLSKD_PAGE = 'https://github.com/slskd/slskd';
const GUIDE = 'https://github.com/dillinger445-alt/sonora/blob/main/docs/SOULSEEK.md';

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

const STORE_NAME = { keychain: 'your keychain', 'credential-manager': 'Windows Credential Manager', file: 'SONORA’s data folder' } as const;

/**
 * The lines to add to slskd.yml for SONORA's key. No `cidr` line: slskd in
 * Docker sees requests from Docker's own address, and slskd.yml's
 * `web.ip_address` is the place to keep its web page to this computer.
 */
export function yamlFor(key: string): string {
  return ['web:', '  authentication:', '    api_keys:', '      sonora:', `        key: ${key}`, '        role: readwrite'].join('\n');
}

export function SoulseekSetup({ state }: { state: SoulseekState }) {
  const { client, bridge, settings, saveSettings } = useCore();
  const toast = useToast();
  const [confirm, dialogs] = useConfirm();
  const status = state.slskd.data;
  const src = state.sources.data;
  const [newKey, setNewKey] = useState<string | null>(null);
  const [pasting, setPasting] = useState(false);
  const [keyText, setKeyText] = useState('');
  const [url, setUrl] = useState(status?.url ?? '');
  const [folderText, setFolderText] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [dialog, setDialog] = useState<'setup' | 'update' | null>(null);
  const [manual, setManual] = useState(false);
  if (!status || !src) return null;

  const run = async (what: string, f: () => Promise<void>) => {
    setBusy(what);
    try {
      await f();
    } finally {
      setBusy(null);
    }
  };

  const createKey = () =>
    run('create', async () => {
      try {
        const r = await client.slskdNewKey();
        setNewKey(r.key);
        setPasting(false);
        state.slskd.reload();
      } catch (e) {
        toast({ tone: 'error', title: 'Could not make a key', body: message(e) });
      }
    });

  const saveKey = () =>
    run('save', async () => {
      try {
        await client.slskdConfigure({ apiKey: keyText.trim() });
        setKeyText('');
        setPasting(false);
        setNewKey(null);
        state.slskd.reload();
        const c = await state.test();
        if (c?.ok) toast({ tone: 'success', title: 'Connected to slskd' });
      } catch (e) {
        toast({ tone: 'error', title: 'Could not save the key', body: message(e) });
      }
    });

  const removeKey = async () => {
    const { confirmed } = await confirm({
      title: 'Remove slskd’s API key?',
      body: <p>Searching and the downloads list stop working until you add a key again. slskd and your downloads are not affected.</p>,
      confirmLabel: 'Remove key',
    });
    if (!confirmed) return;
    try {
      await client.slskdConfigure({ apiKey: null });
      setNewKey(null);
      state.slskd.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not remove the key', body: message(e) });
    }
  };

  const copy = async (text: string) => {
    try {
      await copyText(text);
      toast({ tone: 'success', title: 'Copied' });
    } catch {
      toast({ tone: 'warning', title: 'Could not copy', body: 'Select the lines and copy them with the keyboard.' });
    }
  };

  const saveUrl = () =>
    run('url', async () => {
      try {
        const s = await client.slskdConfigure({ url });
        setUrl(s.url);
        state.slskd.reload();
        if (s.keySaved) await state.test();
      } catch (e) {
        toast({ tone: 'error', title: 'Could not save the address', body: message(e) });
      }
    });

  const setFolder = (folder: string | null) =>
    run('folder', async () => {
      try {
        await client.sourcesConfigure({ folder });
        setFolderText('');
        state.sources.reload();
      } catch (e) {
        toast({ tone: 'error', title: 'Could not use that folder', body: message(e) });
      }
    });

  const chooseFolder = async () => {
    try {
      const p = await bridge.pickFolder('Choose the folder slskd saves finished downloads in');
      if (p) await setFolder(p);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not open the folder picker', body: message(e) });
    }
  };

  const addExisting = () =>
    run('existing', async () => {
      try {
        const r = await client.sourcesAddExisting();
        state.sources.reload();
        toast({ tone: 'success', title: src.review ? `${plural(r.added, 'file')} waiting for your review in Downloads` : `Adding ${plural(r.added, 'file')} to the library` });
      } catch (e) {
        toast({ tone: 'error', title: 'Could not add the files', body: message(e) });
      }
    });

  const checkNow = () =>
    run('check', async () => {
      try {
        const r = await client.sourcesCheck();
        state.sources.reload();
        if (r.error) toast({ tone: 'warning', title: 'The download folder could not be read', body: r.error });
        else if (r.found.imported + r.found.held > 0)
          toast({ tone: 'success', title: r.found.held > 0 ? `${plural(r.found.held, 'download')} waiting for your review` : `Adding ${plural(r.found.imported, 'download')} to the library` });
        else toast({ tone: 'info', title: 'Nothing new to add', body: r.found.settling > 0 ? `${plural(r.found.settling, 'file')} still changing or just arrived; SONORA takes them once they settle.` : undefined });
      } catch (e) {
        toast({ tone: 'error', title: 'Could not check the folder', body: message(e) });
      }
    });

  const c = state.connection;
  const suggestions = status.defaultFolders.filter((f) => f !== src.folder);
  const windows = bridge.platform === 'windows';
  const m = state.managed.data;
  const setupRun = m?.setup ?? null;
  // SONORA runs slskd here: the manual steps are only for looking things up.
  const managed = !!m?.supported && m.configured;
  const showManual = !m?.supported || manual || (!m.configured && status.keySaved) || (managed && manual);
  const n = (i: number, title: string) => (managed ? title : `${i}. ${title}`);

  return (
    <div className="settings sk-setup">
      {m?.supported && setupRun && (setupRun.active || setupRun.error || setupRun.finished) ? (
        <SetupProgress run={setupRun} others={m.others} onChanged={state.managed.reload} />
      ) : null}
      {m?.supported && !m.configured && !setupRun?.active ? <SetupCard onStart={() => setDialog('setup')} onManual={() => setManual(true)} /> : null}
      {m && managed ? <ManagedPanel m={m} state={state} onUpdate={() => setDialog('update')} /> : null}
      {m?.supported ? (
        <SetupDialog open={dialog != null} update={dialog === 'update'} onClose={() => setDialog(null)} onStarted={() => state.managed.reload()} />
      ) : null}
      {managed && !manual ? (
        <p className="sk-gap">
          <button type="button" className="link" onClick={() => setManual(true)}>
            Show the connection details
          </button>
        </p>
      ) : null}
      {showManual ? (
        <>
      <Group
        title={managed ? 'Connection details' : '1. slskd'}
        description="A free Soulseek client that runs in the background, with its own web page for sharing, chat and settings. SONORA searches and downloads through it."
      >
        <Row
          label="Install slskd and sign in to Soulseek"
          description={
            <>
              Download slskd for your system, put your Soulseek user name and password in its settings file (slskd.yml) and start it. What slskd shares is set there too;
              SONORA never changes it. The setup guide goes through each step.
              {windows ? (
                <span className="t-amber sk-block">
                  Soulseek’s rules allow clients other than the official one only on systems other than Windows. On Windows, run slskd on another computer (a Mac, a Linux
                  machine or a NAS) and enter its address below, or use the official SoulseekQt client with the download folder in step 4.
                </span>
              ) : null}
            </>
          }
        >
          <Button size="sm" icon={LuExternalLink} onClick={() => void bridge.openExternal(SLSKD_PAGE).catch(() => undefined)}>
            Get slskd
          </Button>
          <Button size="sm" variant="ghost" icon={LuExternalLink} onClick={() => void bridge.openExternal(GUIDE).catch(() => undefined)}>
            Setup guide
          </Button>
        </Row>
      </Group>

      <Group title={n(2, 'API key')} description={`SONORA signs its requests to slskd with a key, kept in ${STORE_NAME[status.keyStore]}. slskd accepts the key once it is in slskd.yml.`}>
        {newKey ? (
          <div className="sk-key">
            <p>
              Add these lines to slskd.yml (merge them into its <span className="sk-code">web</span> section if it has one), save the file and restart slskd.
            </p>
            <pre className="sk-yaml selectable" aria-label="Lines for slskd.yml">
              {yamlFor(newKey)}
            </pre>
            <div className="sk-key-actions">
              <Button size="sm" icon={LuCopy} onClick={() => void copy(yamlFor(newKey))}>
                Copy lines
              </Button>
              <Button size="sm" variant="primary" busy={state.testing} onClick={() => void state.test()}>
                I restarted slskd: test
              </Button>
            </div>
          </div>
        ) : null}
        {status.keySaved && !newKey && !pasting ? (
          <Row label="Key saved" description={status.keyError ? <span className="t-amber">{status.keyError}</span> : `Kept in ${STORE_NAME[status.keyStore]}.`}>
            <Button size="sm" onClick={() => setPasting(true)}>
              Replace
            </Button>
            <Button size="sm" variant="ghost" className="is-danger-text" onClick={() => void removeKey()}>
              Remove
            </Button>
          </Row>
        ) : null}
        {!status.keySaved || pasting ? (
          <Row
            label={pasting || status.keySaved ? 'Use a different key' : 'No key yet'}
            description="Make a new key, or paste one that is already in slskd.yml (16 to 255 characters)."
          >
            {!newKey ? (
              <Button size="sm" variant="primary" icon={LuKeyRound} busy={busy === 'create'} onClick={() => void createKey()}>
                Make a key
              </Button>
            ) : null}
            <form
              className="discogs-form"
              onSubmit={(e) => {
                e.preventDefault();
                void saveKey();
              }}
            >
              <input
                className="text-input"
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder="Paste a key"
                aria-label="slskd API key"
                value={keyText}
                onChange={(e) => setKeyText(e.target.value)}
              />
              <Button size="sm" type="submit" busy={busy === 'save'} disabled={keyText.trim().length < 16}>
                Save key
              </Button>
            </form>
          </Row>
        ) : null}
      </Group>

      <Group title={n(3, 'Connection')} description="Where slskd’s web page answers. It is on this computer unless you run slskd elsewhere.">
        <Row
          label="slskd’s address"
          description={
            status.local
              ? 'Usually http://localhost:5030.'
              : 'slskd is on another computer. Over http:// the key travels unencrypted on your network, and slskd’s own https certificate changes each time it starts, so SONORA cannot check it. Keep slskd on this computer, or give it a certificate your system trusts and use https://.'
          }
        >
          <form
            className="discogs-form"
            onSubmit={(e) => {
              e.preventDefault();
              void saveUrl();
            }}
          >
            <input className="text-input" spellCheck={false} autoComplete="off" aria-label="slskd’s address" value={url} onChange={(e) => setUrl(e.target.value)} />
            <Button size="sm" type="submit" busy={busy === 'url'} disabled={url.trim() === status.url || !url.trim()}>
              Save
            </Button>
          </form>
        </Row>
        <Row
          label="Test the connection"
          description={
            !status.keySaved ? (
              'Save a key first.'
            ) : !c ? (
              'Not tested yet.'
            ) : c.ok ? (
              <span>
                <span className={c.info.loggedIn ? 't-lime' : 't-amber'}>{c.info.loggedIn ? 'Connected.' : 'slskd answers, but it is not signed in to Soulseek.'}</span> slskd{' '}
                {c.info.version}
                {c.info.username ? `, Soulseek user ${c.info.username}` : ''}
                {c.info.sharedFiles != null ? `, sharing ${plural(c.info.sharedFiles, 'file')} in ${plural(c.info.sharedFolders ?? 0, 'folder')}` : ''}.
              </span>
            ) : (
              <span className="t-amber">{c.error}</span>
            )
          }
        >
          <Button size="sm" icon={LuRefreshCw} busy={state.testing} disabled={!status.keySaved} onClick={() => void state.test()}>
            Test
          </Button>
        </Row>
      </Group>

        </>
      ) : null}

      <Group
        title={n(4, 'Download folder')}
        description="Where slskd puts finished downloads. SONORA adds each finished file to your library from there, and never moves, renames or deletes anything in it."
      >
        <Row
          label={src.folder ? 'Watching' : 'No folder chosen'}
          description={
            src.folder ? (
              <>
                <span className="selectable mono-path">{src.folder}</span>
                {src.error ? <span className="t-amber sk-block">{src.error}</span> : null}
                {src.lastCheck ? <span className="sk-block">Last looked {formatRelative(src.lastCheck)}.</span> : null}
              </>
            ) : (
              'Choose slskd’s download folder (directories → downloads in slskd.yml). Using SoulseekQt or Nicotine+ instead? Choose the folder it saves to; only searching needs slskd.'
            )
          }
        >
          {bridge.native ? (
            <Button size="sm" icon={LuFolderOpen} busy={busy === 'folder'} onClick={() => void chooseFolder()}>
              {src.folder ? 'Change…' : 'Choose…'}
            </Button>
          ) : (
            <form
              className="discogs-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (isAbsolutePath(folderText.trim())) void setFolder(folderText.trim());
                else toast({ tone: 'warning', title: 'Enter the full path of the folder' });
              }}
            >
              <input
                className="text-input selectable"
                placeholder="/full/path/to/downloads"
                aria-label="Download folder path"
                value={folderText}
                onChange={(e) => setFolderText(e.target.value)}
              />
              <Button size="sm" type="submit" busy={busy === 'folder'} disabled={!folderText.trim()}>
                Use folder
              </Button>
            </form>
          )}
          {src.folder ? (
            <Button size="sm" icon={LuRefreshCw} busy={busy === 'check'} onClick={() => void checkNow()}>
              Check now
            </Button>
          ) : null}
        </Row>
        {src.folder && src.existing > 0 ? (
          <Row
            label="Files that were already there"
            description={`${plural(src.existing, 'audio file')} not in your library ${src.existing === 1 ? 'was' : 'were'} in this folder before you chose it. ${src.existing === 1 ? 'It stays' : 'They stay'} out of your library, unmarked, unless you add ${src.existing === 1 ? 'it' : 'them'}.`}
          >
            <Button size="sm" busy={busy === 'existing'} onClick={() => void addExisting()}>
              {src.existing === 1 ? 'Add it' : 'Add them'}
            </Button>
          </Row>
        ) : null}
        {suggestions.map((f) => (
          <Row key={f} label="slskd’s folder on this computer" description={<span className="selectable mono-path">{f}</span>}>
            <Button size="sm" busy={busy === 'folder'} onClick={() => void setFolder(f)}>
              Use this folder
            </Button>
          </Row>
        ))}
        <Switch
          label="Review each download before it joins the library"
          description="Finished files wait in Downloads until you add them or keep them out."
          checked={src.review}
          onChange={(v) =>
            void client.sourcesConfigure({ review: v }).then(
              () => state.sources.reload(),
              (e: unknown) => toast({ tone: 'error', title: 'Could not save the setting', body: message(e) }),
            )
          }
        />
        {src.folder ? (
          <Row label="Stop watching the folder" description={`${plural(src.counts.imported, 'download')} added so far. Tracks already added stay in your library.`}>
            <Button size="sm" variant="ghost" onClick={() => void setFolder(null)}>
              Stop watching
            </Button>
          </Row>
        ) : null}
      </Group>

      <Group
        title="Previews"
        description="A preview asks slskd for the file and plays it as it arrives. It reads only so far ahead of what plays, stops fetching when you stop listening, and keeps nothing."
      >
        <Row label="Read ahead" description="How much a preview fetches beyond what is playing. More rides out a slow connection; less fetches less of what you may not listen to.">
          <Segmented
            label="Read ahead"
            size="sm"
            value={String(settings?.previewAhead ?? 60)}
            onChange={(v) => void saveSettings({ previewAhead: Number(v) }).catch((e: unknown) => toast({ tone: 'error', title: 'Could not save the setting', body: message(e) }))}
            options={[
              { value: '30', label: '30 s' },
              { value: '60', label: '1 min' },
              { value: '120', label: '2 min' },
            ]}
          />
        </Row>
        <Row label="Space for previews" description="The most disk space a preview’s decoded audio may take. Previews longer than this stop where it runs out.">
          <Segmented
            label="Space for previews"
            size="sm"
            value={String(settings?.previewCacheMb ?? 1024)}
            onChange={(v) => void saveSettings({ previewCacheMb: Number(v) }).catch((e: unknown) => toast({ tone: 'error', title: 'Could not save the setting', body: message(e) }))}
            options={[
              { value: '512', label: '512 MB' },
              { value: '1024', label: '1 GB' },
              { value: '2048', label: '2 GB' },
            ]}
          />
        </Row>
      </Group>

      <Group title="Notice" description={src.acceptedAt ? `Accepted ${formatDate(src.acceptedAt)}.` : undefined}>
        <div className="privacy">
          <LuShieldCheck className="privacy-icon" aria-hidden />
          <div>
            <p>{LAWFUL_USE}</p>
            <p className="faint">{SEARCHES_SEEN}</p>
            <p className="faint">
              SONORA sends slskd the words you search for and the files you choose. It keeps where each download came from (Soulseek), its original name and when it arrived,
              and nothing about other people.{src.counts.imported > 0 ? ` ${plural(src.counts.imported, 'track')} came from Soulseek so far.` : ''}
            </p>
          </div>
        </div>
      </Group>
      {dialogs}
    </div>
  );
}
