/**
 * Playlists for the whole window: the tree (sidebar, menus, dialogs) and
 * what can be done with it. Changes go to the core, which announces them
 * (`playlists.changed`), and every view reloads from there.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import type { LibraryFilter, PlaylistKind, PlaylistNode, SmartRules, Sort } from '../api/types.ts';
import { useConfirm } from '../components/Dialog.tsx';
import { plural } from '../lib/format.ts';
import { readPref, writePref } from '../lib/persist.ts';
import { reinsertions } from '../lib/reinsert.ts';
import { navigate, parseRoute } from '../lib/router.ts';
import { OrganizeDialog, type OrganizeTarget } from '../views/OrganizeDialog.tsx';
import { SmartRulesDialog, type RulesTarget } from '../views/SmartRulesDialog.tsx';
import { UsbDialog } from '../views/UsbDialog.tsx';
import { useCore } from './core.tsx';
import { useResource } from './useResource.ts';
import { useToast } from './toasts.tsx';

export interface Playlists {
  nodes: PlaylistNode[];
  byId: Map<number, PlaylistNode>;
  /** Children of a folder (or the top level), in order. */
  childrenOf: (parent: number | null) => PlaylistNode[];
  loaded: boolean;
  expanded: ReadonlySet<number>;
  setExpanded: (id: number, open: boolean) => void;
  /** The node whose name is being edited in the sidebar. */
  renaming: number | null;
  setRenaming: (id: number | null) => void;
  /** Creates a node with a free default name and starts renaming it. */
  create: (kind: Exclude<PlaylistKind, 'smart'>, opts?: { parentId?: number | null; index?: number; trackIds?: number[]; open?: boolean }) => Promise<PlaylistNode | null>;
  createSmart: (name: string, rules: SmartRules) => Promise<PlaylistNode | null>;
  rename: (id: number, name: string) => Promise<void>;
  move: (id: number, parentId: number | null, index: number) => Promise<void>;
  /** Deletes after asking; true once deleted. */
  remove: (id: number) => Promise<boolean>;
  duplicate: (id: number) => Promise<void>;
  addTracks: (id: number, trackIds: number[], index?: number) => Promise<void>;
  /** Removes, with Undo in the notification. `order` is the order before. */
  removeTracks: (id: number, trackIds: number[], order: number[]) => Promise<void>;
  freeze: (id: number) => Promise<void>;
  /** Folders and the path of names above a node ("Sets / Friday"). */
  pathOf: (id: number) => string;
  /** Whether `id` is `ancestor` or inside it. */
  isWithin: (id: number, ancestor: number) => boolean;
  openOrganize: (target?: OrganizeTarget) => void;
  openUsb: (playlistIds?: number[]) => void;
  openRules: (target: RulesTarget) => void;
  /** Saves library filters as a smart playlist (asks for the name). */
  saveFilters: (filter: LibraryFilter, sort: Sort, suggestedName: string) => void;
}

const PlaylistsContext = createContext<Playlists | null>(null);

export function usePlaylists(): Playlists {
  const v = useContext(PlaylistsContext);
  if (!v) throw new Error('usePlaylists outside PlaylistsProvider');
  return v;
}

const isIds = (v: unknown): v is number[] => Array.isArray(v) && v.every((x) => typeof x === 'number');

function freeName(base: string, taken: Iterable<string>): string {
  const names = new Set([...taken].map((n) => n.toLowerCase()));
  if (!names.has(base.toLowerCase())) return base;
  for (let n = 2; ; n += 1) if (!names.has(`${base} ${n}`.toLowerCase())) return `${base} ${n}`;
}

export function PlaylistsProvider({ children }: { children: ReactNode }) {
  const { client } = useCore();
  const toast = useToast();
  const [confirm, confirmDialog] = useConfirm();
  const list = useResource(() => client.playlists(), [], { events: ['playlists.changed', 'library.changed', 'scan.finished'], throttleMs: 400 });
  // Nodes just made here, shown until the reloaded list holds them.
  const [fresh, setFresh] = useState<PlaylistNode[]>([]);
  const listed = list.data?.nodes;
  useEffect(() => {
    if (listed) setFresh((f) => (f.length ? f.filter((n) => !listed.some((x) => x.id === n.id)) : f));
  }, [listed]);
  const nodes = useMemo(() => {
    const base = listed ?? [];
    if (!fresh.length) return base;
    const ids = new Set(base.map((n) => n.id));
    return [...base, ...fresh.filter((n) => !ids.has(n.id))];
  }, [listed, fresh]);
  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);
  const kids = useMemo(() => {
    const m = new Map<number | null, PlaylistNode[]>();
    for (const n of nodes) {
      const k = n.parentId ?? null;
      m.set(k, [...(m.get(k) ?? []), n]);
    }
    for (const v of m.values()) v.sort((a, b) => a.position - b.position || a.id - b.id);
    return m;
  }, [nodes]);
  const childrenOf = useCallback((parent: number | null) => kids.get(parent) ?? [], [kids]);
  const [expanded, setExpandedState] = useState<Set<number>>(() => new Set(readPref('playlists.expanded', [], isIds)));
  const [renaming, setRenaming] = useState<number | null>(null);
  const [organize, setOrganize] = useState<OrganizeTarget | null>(null);
  const [usb, setUsb] = useState<number[] | null>(null);
  const [rules, setRules] = useState<RulesTarget | null>(null);
  const reload = list.reload;

  const setExpanded = useCallback((id: number, open: boolean) => {
    setExpandedState((s) => {
      if (s.has(id) === open) return s;
      const next = new Set(s);
      if (open) next.add(id);
      else next.delete(id);
      writePref('playlists.expanded', [...next]);
      return next;
    });
  }, []);

  const fail = useCallback((title: string, e: unknown) => toast({ tone: 'error', title, body: e instanceof Error ? e.message : String(e) }), [toast]);

  const create = useCallback<Playlists['create']>(
    async (kind, opts = {}) => {
      const siblings = childrenOf(opts.parentId ?? null).map((n) => n.name);
      const name = freeName(kind === 'folder' ? 'New folder' : 'New playlist', siblings);
      try {
        const node = await client.createPlaylist(kind, name, { parentId: opts.parentId ?? null, index: opts.index, trackIds: opts.trackIds });
        setFresh((f) => [...f, node]);
        if (opts.parentId != null) setExpanded(opts.parentId, true);
        reload();
        setRenaming(node.id);
        if (opts.open !== false && kind === 'playlist') navigate({ view: 'playlist', id: node.id });
        if (opts.trackIds?.length) toast({ tone: 'success', title: `Made a playlist of ${plural(opts.trackIds.length, 'track')}`, body: 'Type its name in the sidebar.' });
        return node;
      } catch (e) {
        fail(kind === 'folder' ? 'Could not make the folder' : 'Could not make the playlist', e);
        return null;
      }
    },
    [client, childrenOf, setExpanded, reload, toast, fail],
  );

  const createSmart = useCallback<Playlists['createSmart']>(
    async (name, r) => {
      try {
        const node = await client.createPlaylist('smart', name, { rules: r });
        setFresh((f) => [...f, node]);
        reload();
        navigate({ view: 'playlist', id: node.id });
        return node;
      } catch (e) {
        fail('Could not make the smart playlist', e);
        return null;
      }
    },
    [client, reload, fail],
  );

  const rename = useCallback(
    async (id: number, name: string) => {
      try {
        await client.updatePlaylist(id, { name });
        reload();
      } catch (e) {
        fail('Could not rename', e);
      }
    },
    [client, reload, fail],
  );

  const isWithin = useCallback(
    (id: number, ancestor: number) => {
      let cur: number | null | undefined = id;
      for (let guard = 0; cur != null && guard < 100; guard += 1) {
        if (cur === ancestor) return true;
        cur = byId.get(cur)?.parentId;
      }
      return false;
    },
    [byId],
  );

  const move = useCallback(
    async (id: number, parentId: number | null, index: number) => {
      try {
        await client.movePlaylist(id, parentId, index);
        if (parentId != null) setExpanded(parentId, true);
        reload();
      } catch (e) {
        fail('Could not move', e);
      }
    },
    [client, reload, setExpanded, fail],
  );

  const remove = useCallback(
    async (id: number) => {
      const n = byId.get(id);
      if (!n) return false;
      const inside = n.kind === 'folder' ? nodes.filter((x) => x.id !== id && isWithin(x.id, id)).length : 0;
      const what = n.kind === 'folder' ? 'folder' : n.kind === 'smart' ? 'smart playlist' : 'playlist';
      const { confirmed } = await confirm({
        title: `Delete the ${what} “${n.name}”?`,
        body: (
          <>
            {n.kind === 'folder' && inside > 0 ? <p>{plural(inside, 'playlist or folder', 'playlists and folders')} inside it go too.</p> : null}
            <p className="faint">The tracks stay in your library and your files stay where they are.</p>
          </>
        ),
        confirmLabel: `Delete ${what}`,
      });
      if (!confirmed) return false;
      try {
        await client.deletePlaylist(id);
        setFresh((f) => f.filter((x) => x.id !== id && !isWithin(x.id, id)));
        reload();
        // Where the window is now, not where it was when this started.
        const now = parseRoute(window.location.hash);
        if (now.view === 'playlist' && isWithin(now.id, id)) navigate({ view: 'library' }, true);
        toast({ tone: 'success', title: `Deleted “${n.name}”` });
        return true;
      } catch (e) {
        fail('Could not delete', e);
        return false;
      }
    },
    [byId, nodes, isWithin, confirm, client, reload, toast, fail],
  );

  const duplicate = useCallback(
    async (id: number) => {
      try {
        const copy = await client.duplicatePlaylist(id);
        setFresh((f) => [...f, copy]);
        reload();
        setRenaming(copy.id);
      } catch (e) {
        fail('Could not duplicate', e);
      }
    },
    [client, reload, fail],
  );

  const addTracks = useCallback(
    async (id: number, trackIds: number[], index?: number) => {
      const n = byId.get(id);
      try {
        const r = await client.addToPlaylist(id, trackIds, index);
        const name = n ? `“${n.name}”` : 'the playlist';
        const now = parseRoute(window.location.hash);
        // Nothing added and nothing there already: the tracks are gone (B93).
        if (r.added === 0 && r.already === 0) toast({ tone: 'warning', title: `Nothing added to ${name}`, body: 'Those tracks are no longer in the library.' });
        else if (r.added === 0) toast({ tone: 'info', title: r.already === 1 ? `Already in ${name}` : `All already in ${name}` });
        else
          toast({
            tone: 'success',
            title: `Added ${plural(r.added, 'track')} to ${name}`,
            body: r.already > 0 ? `${plural(r.already, 'track')} ${r.already === 1 ? 'was' : 'were'} there already.` : undefined,
            action: now.view === 'playlist' && now.id === id ? undefined : { label: 'Open', run: () => navigate({ view: 'playlist', id }) },
          });
      } catch (e) {
        fail('Could not add to the playlist', e);
      }
    },
    [byId, client, toast, fail],
  );

  const removeTracks = useCallback(
    async (id: number, trackIds: number[], order: number[]) => {
      try {
        const r = await client.removeFromPlaylist(id, trackIds);
        toast({
          tone: 'success',
          title: `Removed ${plural(r.removed, 'track')} from the playlist`,
          body: 'They stay in your library.',
          action: {
            label: 'Undo',
            run: () => {
              // Back where they were, even if the playlist changed since.
              void (async () => {
                const now = (await client.playlistTracks(id)).rows.map((t) => t.id);
                for (const step of reinsertions(order, trackIds, now)) await client.addToPlaylist(id, step.ids, step.index);
              })().catch((e: unknown) => fail('Could not undo', e));
            },
          },
          timeout: 8000,
        });
      } catch (e) {
        fail('Could not remove from the playlist', e);
      }
    },
    [client, toast, fail],
  );

  const freeze = useCallback(
    async (id: number) => {
      const n = byId.get(id);
      if (!n) return;
      // The rules are dropped for good: ask first.
      const { confirmed } = await confirm({
        title: `Convert “${n.name}” to a playlist?`,
        body: (
          <p>
            It keeps the tracks that match now, in their current order. New tracks no longer join it by themselves, and its rules are not kept. To keep the rules as well,
            duplicate it first.
          </p>
        ),
        confirmLabel: 'Convert to a playlist',
        tone: 'primary',
      });
      if (!confirmed) return;
      try {
        const r = await client.freezePlaylist(id);
        reload();
        toast({ tone: 'success', title: `Now a playlist of ${plural(r.tracks, 'track')}`, body: 'Its tracks no longer follow the rules; order them as you like.' });
      } catch (e) {
        fail('Could not convert', e);
      }
    },
    [byId, confirm, client, reload, toast, fail],
  );

  const pathOf = useCallback(
    (id: number) => {
      const names: string[] = [];
      let cur = byId.get(id)?.parentId ?? null;
      while (cur != null && names.length < 20) {
        const n = byId.get(cur);
        if (!n) break;
        names.unshift(n.name);
        cur = n.parentId;
      }
      return names.join(' / ');
    },
    [byId],
  );

  const saveFilters = useCallback((filter: LibraryFilter, sort: Sort, suggestedName: string) => {
    setRules({ mode: 'create', name: suggestedName, rules: { filter, sort } });
  }, []);

  const value = useMemo<Playlists>(
    () => ({
      nodes,
      byId,
      childrenOf,
      loaded: list.data != null,
      expanded,
      setExpanded,
      renaming,
      setRenaming,
      create,
      createSmart,
      rename,
      move,
      remove,
      duplicate,
      addTracks,
      removeTracks,
      freeze,
      pathOf,
      isWithin,
      openOrganize: (t) => setOrganize(t ?? { ids: null }),
      openUsb: (ids) => setUsb(ids ?? []),
      openRules: setRules,
      saveFilters,
    }),
    [nodes, byId, childrenOf, list.data, expanded, setExpanded, renaming, create, createSmart, rename, move, remove, duplicate, addTracks, removeTracks, freeze, pathOf, isWithin, saveFilters],
  );

  return (
    <PlaylistsContext.Provider value={value}>
      {children}
      {confirmDialog}
      <OrganizeDialog target={organize} onClose={() => setOrganize(null)} />
      <UsbDialog playlistIds={usb} onClose={() => setUsb(null)} />
      <SmartRulesDialog target={rules} onClose={() => setRules(null)} />
    </PlaylistsContext.Provider>
  );
}
