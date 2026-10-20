/**
 * The optional Discogs connection: a personal access token, kept in the
 * library database on this computer and never shown again once saved.
 */

import { useState } from 'react';
import { LuCircleCheck, LuExternalLink, LuTriangleAlert } from 'react-icons/lu';
import { Button } from '../components/Button.tsx';
import { useConfirm } from '../components/Dialog.tsx';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import { useResource } from '../state/useResource.ts';

const TOKEN_PAGE = 'https://www.discogs.com/settings/developers';

export function DiscogsSettings() {
  const { client, bridge } = useCore();
  const toast = useToast();
  const [confirm, confirmDialog] = useConfirm();
  const status = useResource(() => client.discogsStatus(), [], { events: ['discogs.finished'] });
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState<'save' | 'test' | null>(null);
  /** The last check of the saved token: whose it is, or that Discogs refused it or could not be reached. */
  const [check, setCheck] = useState<{ ok: true; user: string } | { ok: false } | null>(null);
  const configured = status.data?.configured ?? false;

  const save = async () => {
    const t = token.trim();
    if (!t) return;
    setBusy('save');
    try {
      await client.discogsSetToken(t);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not save the token', body: e instanceof Error ? e.message : String(e) });
      setBusy(null);
      return;
    }
    setToken('');
    status.reload();
    // Check it right away, so a typo shows now rather than at the first lookup.
    try {
      const r = await client.discogsTest();
      setCheck({ ok: true, user: r.username });
      toast({ tone: 'success', title: `Connected to Discogs as ${r.username}` });
    } catch (e) {
      setCheck({ ok: false });
      toast({ tone: 'error', title: 'The token is saved, but the check failed', body: e instanceof Error ? e.message : String(e), timeout: 0 });
    } finally {
      setBusy(null);
    }
  };

  const test = async () => {
    setBusy('test');
    try {
      const r = await client.discogsTest();
      setCheck({ ok: true, user: r.username });
    } catch (e) {
      setCheck({ ok: false });
      toast({ tone: 'error', title: 'The check failed', body: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    const { confirmed } = await confirm({
      title: 'Remove the Discogs token?',
      body: <p>Genre lookups stop working until you add a token again. Genres already found stay in the library.</p>,
      confirmLabel: 'Remove token',
    });
    if (!confirmed) return;
    try {
      await client.discogsSetToken(null);
      setCheck(null);
      status.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not remove the token', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const openTokenPage = () => void bridge.openExternal(TOKEN_PAGE).catch(() => undefined);

  return (
    <div className="discogs">
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Discogs genre lookup</span>
          <span className="switch-desc">
            Optional. For tracks you choose, SONORA searches Discogs by artist and title (up to three searches a track, one a second), each search signed with your token.
            Saving or testing the token sends it to Discogs once to check it. Nothing else leaves this computer. It needs a free Discogs account and a personal access token.
          </span>
        </div>
        <div className="setting-control discogs-control">
          {configured ? (
            <>
              <span className="discogs-state">
                {check?.ok === false ? <LuTriangleAlert aria-hidden className="t-amber" /> : <LuCircleCheck aria-hidden className="t-lime" />}
                {check?.ok ? `Connected as ${check.user}` : check ? 'Token saved, not working' : 'Token saved'}
              </span>
              <Button size="sm" busy={busy === 'test'} onClick={() => void test()}>
                Test
              </Button>
              <Button size="sm" variant="ghost" className="is-danger-text" onClick={() => void remove()}>
                Remove
              </Button>
            </>
          ) : (
            <form
              className="discogs-form"
              onSubmit={(e) => {
                e.preventDefault();
                void save();
              }}
            >
              <input
                className="text-input"
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder="Personal access token"
                aria-label="Discogs personal access token"
                value={token}
                onChange={(e) => setToken(e.target.value)}
              />
              <Button size="sm" type="submit" variant="primary" busy={busy === 'save'} disabled={!token.trim()}>
                Save
              </Button>
            </form>
          )}
        </div>
      </div>
      {!configured ? (
        <p className="discogs-help faint">
          Create a token on Discogs under Settings → Developers.{' '}
          <button type="button" className="link" onClick={openTokenPage}>
            Open the Discogs page <LuExternalLink aria-hidden className="inline-icon" />
          </button>
        </p>
      ) : null}
      <p className="discogs-legal faint">
        Genres looked up this way: data provided by Discogs. This application uses Discogs’ API but is not affiliated with, sponsored or endorsed by Discogs. “Discogs” is a
        trademark of Zink Media, LLC.
      </p>
      {confirmDialog}
    </div>
  );
}
