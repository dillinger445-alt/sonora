/**
 * Hash-based routing (works the same in the desktop webview and in a
 * browser, and keeps back/forward navigation).
 */

import { useSyncExternalStore } from 'react';

export type Route =
  | { view: 'overview' }
  | { view: 'library'; folderId?: number }
  | { view: 'queue' }
  | { view: 'folders' }
  | { view: 'attention'; flag?: string }
  | { view: 'duplicates' }
  | { view: 'history'; scanId?: number }
  | { view: 'settings' }
  | { view: 'track'; id: number }
  | { view: 'playlist'; id: number };

export type View = Route['view'];

const SIMPLE: View[] = ['overview', 'queue', 'folders', 'duplicates', 'settings'];

export function parseRoute(hash: string): Route {
  const clean = hash.replace(/^#\/?/, '');
  const [path = '', query = ''] = clean.split('?');
  const parts = path.split('/').filter(Boolean);
  const head = parts[0] ?? 'overview';
  const params = new URLSearchParams(query);
  if (head === 'track') {
    const id = Number(parts[1]);
    return Number.isInteger(id) && id > 0 ? { view: 'track', id } : { view: 'library' };
  }
  if (head === 'playlist') {
    const id = Number(parts[1]);
    return Number.isInteger(id) && id > 0 ? { view: 'playlist', id } : { view: 'library' };
  }
  if (head === 'history') {
    const id = Number(parts[1]);
    return Number.isInteger(id) && id > 0 ? { view: 'history', scanId: id } : { view: 'history' };
  }
  if (head === 'library') {
    const folder = Number(params.get('folder'));
    return Number.isInteger(folder) && folder > 0 ? { view: 'library', folderId: folder } : { view: 'library' };
  }
  if (head === 'attention') {
    const flag = params.get('flag');
    return flag ? { view: 'attention', flag } : { view: 'attention' };
  }
  if ((SIMPLE as string[]).includes(head)) return { view: head as Exclude<View, 'track' | 'history' | 'attention' | 'library' | 'playlist'> } as Route;
  return { view: 'overview' };
}

export function formatRoute(r: Route): string {
  switch (r.view) {
    case 'track':
      return `#/track/${r.id}`;
    case 'playlist':
      return `#/playlist/${r.id}`;
    case 'history':
      return r.scanId ? `#/history/${r.scanId}` : '#/history';
    case 'attention':
      return r.flag ? `#/attention?flag=${encodeURIComponent(r.flag)}` : '#/attention';
    case 'library':
      return r.folderId ? `#/library?folder=${r.folderId}` : '#/library';
    default:
      return `#/${r.view}`;
  }
}

function subscribe(cb: () => void): () => void {
  window.addEventListener('hashchange', cb);
  return () => window.removeEventListener('hashchange', cb);
}

const getHash = () => window.location.hash;

export function useRoute(): Route {
  const hash = useSyncExternalStore(subscribe, getHash, () => '');
  return parseRoute(hash);
}

export function navigate(r: Route, replace = false): void {
  const next = formatRoute(r);
  if (window.location.hash === next) return;
  if (replace) {
    window.history.replaceState(null, '', next);
    window.dispatchEvent(new HashChangeEvent('hashchange'));
  } else {
    window.location.hash = next;
  }
}
