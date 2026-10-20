/**
 * A stand-in for slskd's web API, for the end-to-end tests: SONORA talks to
 * it exactly as it talks to slskd, and nothing reaches Soulseek. Searches
 * answer from a fixed set of shared folders; downloads wait until the test
 * says they finished.
 */

import { randomUUID } from 'node:crypto';
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, renameSync, statSync, writeSync } from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { basename, dirname, join } from 'node:path';

export interface SharedFile {
  username: string;
  filename: string;
  size: number;
  bitDepth?: number;
  sampleRate?: number;
  bitRate?: number;
  length?: number;
  /** A local file whose bytes the "other user" sends (previews and downloads of it). */
  source?: string;
}

/** Where slskd keeps a partial download: <incomplete>/<user>/<remote folders>/<name>, each part made safe (as on Linux). */
export function incompletePath(root: string, username: string, filename: string): string {
  const safe = (seg: string) => {
    const s = seg.replace(/[\0/\\]/g, '_');
    return s === '.' || s === '..' ? '_' : s;
  };
  const local = filename.replace(/\\/g, '/');
  const unrooted = local
    .replace(/^[a-zA-Z]:\/?/, '')
    .replace(/^\/\/[^/]+\/?/, '')
    .replace(/^@@[a-zA-Z0-9]{5,}\/?/, '');
  const parts = unrooted.split('/').filter(Boolean);
  const name = safe(local.split('/').pop() ?? '');
  return join(root, safe(username), ...parts.slice(0, -1).map(safe), name);
}

interface FakeTransfer {
  id: string;
  username: string;
  filename: string;
  size: number;
  state: string;
  requestedAt: string;
  endedAt: string | null;
  bytesTransferred: number;
  placeInQueue: number | null;
  batchId?: string;
  /** The batch's folder (inside the download folder), for previews. */
  destination?: string;
  removed?: boolean;
  /** Where it resumed from. */
  offset?: number;
}

export interface FakeSlskd {
  url: string;
  /** The API key slskd accepts (set it to the one SONORA made). */
  key: string;
  shared: SharedFile[];
  searches: string[];
  transfers: FakeTransfer[];
  /** Marks a download as finished (the test puts the file in the download folder). */
  finish: (filename: string) => void;
  close: () => Promise<void>;
}

const directory = (filename: string) => filename.slice(0, Math.max(filename.lastIndexOf('\\'), 0));

/**
 * With `config`, the fake behaves like the slskd SONORA set up: it takes the
 * API key from that slskd.yml (as slskd does) and signs in only when the
 * Soulseek password there is "right".
 */
export interface FakeOptions {
  port?: number;
  config?: string;
  /** slskd's folders: where finished and partial downloads go (files with a `source` are "received" there). */
  downloads?: string;
  incomplete?: string;
  /** Bytes "received" every 50 ms. */
  rate?: number;
}

/** A value from the slskd.yml SONORA writes (single-quoted scalars). */
function yamlValue(text: string, pattern: RegExp): string | null {
  const m = pattern.exec(text);
  return m?.[1] != null ? m[1].replace(/''/g, "'") : null;
}

export async function startFakeSlskd(shared: SharedFile[], options: FakeOptions = {}): Promise<FakeSlskd> {
  const fake = { key: '', shared, searches: [] as string[], transfers: [] as FakeTransfer[] };
  const fromConfig = () => {
    if (!options.config || !existsSync(options.config)) return { key: '', password: null as string | null, username: null as string | null };
    const text = readFileSync(options.config, 'utf8');
    return {
      key: yamlValue(text, /^ {8}key: '((?:[^']|'')*)'/m) ?? '',
      password: yamlValue(text, /^soulseek:\n {2}username: '(?:[^']|'')*'\n {2}password: '((?:[^']|'')*)'/m),
      username: yamlValue(text, /^soulseek:\n {2}username: '((?:[^']|'')*)'/m),
    };
  };
  const started = new Map<string, { text: string; at: number }>();

  const stops = new Map<string, () => void>();
  /** "Receives" a shared file into the partial file, from what is there on, as slskd resumes. */
  const receive = (t: FakeTransfer) => {
    const file = fake.shared.find((f) => f.username === t.username && f.filename === t.filename);
    if (!file?.source || !options.incomplete || !options.downloads) return;
    const data = readFileSync(file.source);
    const partial = incompletePath(options.incomplete, t.username, t.filename);
    let pos = 0;
    let fd: number | null = null;
    let stopped = false;
    stops.set(t.id, () => {
      stopped = true;
    });
    const close = () => {
      if (fd != null) closeSync(fd);
      fd = null;
    };
    const step = () => {
      if (stopped) {
        close();
        if (!t.state.startsWith('Completed')) t.state = 'Completed, Cancelled';
        return;
      }
      if (fd == null) {
        // As slskd does when the transfer starts: the folder is made, the
        // partial file opened (and kept open), and what is there already is
        // not asked for again.
        mkdirSync(dirname(partial), { recursive: true });
        pos = existsSync(partial) ? statSync(partial).size : 0;
        t.offset = pos;
        fd = openSync(partial, 'a');
      }
      t.state = 'InProgress';
      t.placeInQueue = null;
      const n = Math.min(options.rate ?? 64 * 1024, data.length - pos);
      writeSync(fd, data.subarray(pos, pos + n));
      pos += n;
      t.bytesTransferred = pos;
      if (pos < data.length) {
        setTimeout(step, 50);
        return;
      }
      close();
      const dir = join(options.downloads ?? '', t.destination ?? 'received');
      mkdirSync(dir, { recursive: true });
      renameSync(partial, join(dir, basename(partial)));
      t.state = 'Completed, Succeeded';
      t.endedAt = new Date().toISOString();
    };
    setTimeout(step, 150);
  };
  const busy = (username: string, filename: string) => fake.transfers.some((x) => x.username === username && x.filename === filename && !x.removed && !x.state.startsWith('Completed'));
  const listed = () => fake.transfers.filter((x) => !x.removed);

  const send = (res: ServerResponse, status: number, body?: unknown) => {
    res.writeHead(status, { 'Content-Type': 'application/json' });
    res.end(body === undefined ? '' : JSON.stringify(body));
  };

  const read = (req: IncomingMessage) =>
    new Promise<string>((resolve) => {
      let b = '';
      req.on('data', (c: Buffer) => (b += c.toString('utf8')));
      req.on('end', () => resolve(b));
    });

  const server = createServer((req, res) => {
    void (async () => {
      const body = await read(req);
      const cfg = fromConfig();
      const key = options.config ? cfg.key : fake.key;
      if (!key || req.headers['x-api-key'] !== key) return send(res, 401);
      const url = new URL(req.url ?? '/', 'http://fake');
      const path = url.pathname.replace(/^\/api\/v0\//, '');
      const method = req.method ?? 'GET';

      if (method === 'GET' && path === 'application') {
        const signedIn = !options.config || cfg.password === 'right';
        return send(res, 200, {
          version: { current: '0.26.1' },
          server: signedIn ? { state: 'Connected, LoggedIn', isConnected: true, isLoggedIn: true } : { state: 'Disconnected', isConnected: false, isLoggedIn: false },
          user: { username: options.config ? cfg.username : 'e2e-dj' },
          shares: { directories: 2, files: 40 },
        });
      }
      if (method === 'POST' && path === 'searches') {
        const text = (JSON.parse(body) as { searchText: string }).searchText;
        const id = randomUUID();
        fake.searches.push(text);
        started.set(id, { text, at: Date.now() });
        return send(res, 200, { id, searchText: text, state: 'InProgress' });
      }
      const search = /^searches\/([0-9a-f-]{36})$/.exec(path)?.[1];
      if (search) {
        const s = started.get(search);
        if (!s) return send(res, 404);
        if (method === 'PUT') return send(res, 200);
        // Answers arrive over a second or so, as they do from the network.
        // Like slskd, the responses are stored only once the search has
        // ended (endedAt), a moment after it says "Completed".
        const elapsed = Date.now() - s.at;
        const completed = elapsed > 1200;
        const ended = elapsed > 1800;
        const users = [...new Set(fake.shared.map((f) => f.username))];
        const answering = completed ? users : users.slice(0, 1);
        const responses = answering.map((username) => ({
          username,
          hasFreeUploadSlot: username === users[0],
          queueLength: username === users[0] ? 0 : 14,
          uploadSpeed: username === users[0] ? 2_400_000 : 90_000,
          fileCount: fake.shared.filter((f) => f.username === username).length,
          files: fake.shared.filter((f) => f.username === username).map(({ username: _u, ...f }) => ({ ...f, extension: '' })),
          lockedFiles: [],
        }));
        return send(res, 200, {
          id: search,
          searchText: s.text,
          state: completed ? 'Completed, TimedOut' : 'InProgress',
          responseCount: responses.length,
          ...(ended ? { endedAt: new Date(s.at + 1800).toISOString() } : {}),
          responses: ended && url.searchParams.get('includeResponses') === 'true' ? responses : [],
        });
      }
      if (method === 'GET' && path === 'options') {
        return send(res, 200, {
          directories: { downloads: options.downloads ?? '', incomplete: options.incomplete ?? '' },
          transfers: { download: { retry: { partial: 'resume' } } },
        });
      }
      if (method === 'POST' && path === 'transfers/downloads/batches') {
        const b = JSON.parse(body) as { id: string; username: string; files: { filename: string; size: number }[]; options?: { destination?: string } };
        const f = b.files[0];
        if (!f) return send(res, 400);
        if (busy(b.username, f.filename)) return send(res, 200, { batch: {}, failures: [{ filename: f.filename, message: 'already in progress' }] });
        const t: FakeTransfer = {
          id: randomUUID(),
          username: b.username,
          filename: f.filename,
          size: f.size,
          state: 'Queued, Remotely',
          requestedAt: new Date().toISOString(),
          endedAt: null,
          bytesTransferred: 0,
          placeInQueue: 1,
          batchId: b.id,
          destination: b.options?.destination,
        };
        fake.transfers.push(t);
        receive(t);
        return send(res, 201, { batch: { id: b.id, transfers: [t] }, failures: [] });
      }
      const one = /^transfers\/downloads\/([^/]+)\/([0-9a-f-]{36})$/.exec(path);
      if (method === 'GET' && one) {
        const t = listed().find((x) => x.id === one[2]);
        return t ? send(res, 200, t) : send(res, 404);
      }
      const user = /^transfers\/downloads\/([^/]+)$/.exec(path);
      if (method === 'GET' && user && user[1] !== 'batches') {
        const username = decodeURIComponent(user[1] ?? '');
        const mine = listed().filter((x) => x.username === username);
        if (mine.length === 0) return send(res, 404);
        return send(res, 200, { username, directories: [{ directory: 'x', fileCount: mine.length, files: mine }] });
      }
      if (method === 'POST' && path.startsWith('transfers/downloads/')) {
        const username = decodeURIComponent(path.slice('transfers/downloads/'.length));
        const files = JSON.parse(body) as { filename: string; size: number }[];
        const enqueued = files.map((f, i) => {
          const t: FakeTransfer = {
            id: randomUUID(),
            username,
            filename: f.filename,
            size: f.size,
            state: 'Queued, Remotely',
            requestedAt: new Date().toISOString(),
            endedAt: null,
            bytesTransferred: 0,
            placeInQueue: i + 2,
          };
          fake.transfers.push(t);
          receive(t);
          return t;
        });
        return send(res, 201, { enqueued, failed: [] });
      }
      if (method === 'GET' && path === 'transfers/downloads') {
        const users = [...new Set(listed().map((t) => t.username))];
        return send(
          res,
          200,
          users.map((username) => {
            const mine = listed().filter((t) => t.username === username);
            const dirs = [...new Set(mine.map((t) => directory(t.filename)))];
            return {
              username,
              directories: dirs.map((d) => ({
                directory: d,
                fileCount: mine.filter((t) => directory(t.filename) === d).length,
                files: mine.filter((t) => directory(t.filename) === d).map((t) => ({ ...t, direction: 'Download', averageSpeed: 1_000_000 })),
              })),
            };
          }),
        );
      }
      if (method === 'DELETE' && path === 'transfers/downloads/all/completed') {
        fake.transfers = fake.transfers.filter((t) => !t.state.startsWith('Completed'));
        return send(res, 204);
      }
      const cancel = /^transfers\/downloads\/[^/]+\/([0-9a-f-]{36})$/.exec(path)?.[1];
      if (method === 'DELETE' && cancel) {
        const t = fake.transfers.find((x) => x.id === cancel);
        if (!t) return send(res, 404);
        stops.get(t.id)?.();
        if (!t.state.startsWith('Completed')) t.state = 'Completed, Cancelled';
        if (url.searchParams.get('remove') === 'true') t.removed = true;
        return send(res, 204);
      }
      return send(res, 404);
    })();
  });
  await new Promise<void>((resolve) => server.listen(options.port ?? 0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  return Object.assign(fake, {
    url: `http://127.0.0.1:${port}`,
    finish: (filename: string) => {
      for (const t of fake.transfers.filter((x) => x.filename === filename)) {
        t.state = 'Completed, Succeeded';
        t.bytesTransferred = t.size;
        t.placeInQueue = null;
        t.endedAt = new Date().toISOString();
      }
    },
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  });
}
