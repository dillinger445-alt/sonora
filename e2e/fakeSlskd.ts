/**
 * A stand-in for slskd's web API, for the end-to-end tests: SONORA talks to
 * it exactly as it talks to slskd, and nothing reaches Soulseek. Searches
 * answer from a fixed set of shared folders; downloads wait until the test
 * says they finished.
 */

import { randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

export interface SharedFile {
  username: string;
  filename: string;
  size: number;
  bitDepth?: number;
  sampleRate?: number;
  bitRate?: number;
  length?: number;
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

export async function startFakeSlskd(shared: SharedFile[]): Promise<FakeSlskd> {
  const fake = { key: '', shared, searches: [] as string[], transfers: [] as FakeTransfer[] };
  const started = new Map<string, { text: string; at: number }>();

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
      if (!fake.key || req.headers['x-api-key'] !== fake.key) return send(res, 401);
      const url = new URL(req.url ?? '/', 'http://fake');
      const path = url.pathname.replace(/^\/api\/v0\//, '');
      const method = req.method ?? 'GET';

      if (method === 'GET' && path === 'application') {
        return send(res, 200, {
          version: { current: '0.26.1' },
          server: { state: 'Connected, LoggedIn', isConnected: true, isLoggedIn: true },
          user: { username: 'e2e-dj' },
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
        const done = Date.now() - s.at > 1200;
        const users = [...new Set(fake.shared.map((f) => f.username))];
        const answering = done ? users : users.slice(0, 1);
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
          state: done ? 'Completed, TimedOut' : 'InProgress',
          responseCount: responses.length,
          responses: url.searchParams.get('includeResponses') === 'true' ? responses : [],
        });
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
          return t;
        });
        return send(res, 201, { enqueued, failed: [] });
      }
      if (method === 'GET' && path === 'transfers/downloads') {
        const users = [...new Set(fake.transfers.map((t) => t.username))];
        return send(
          res,
          200,
          users.map((username) => {
            const mine = fake.transfers.filter((t) => t.username === username);
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
        if (!t.state.startsWith('Completed')) t.state = 'Completed, Cancelled';
        return send(res, 204);
      }
      return send(res, 404);
    })();
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
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
