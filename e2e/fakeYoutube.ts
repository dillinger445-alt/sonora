/**
 * A stand-in for YouTube's playlist page, for the end-to-end tests: it
 * serves a saved page (e2e/fixtures/youtube-playlist.html) for one
 * playlist id and says the others are private, the way YouTube does.
 * The dev bridge is pointed at it with SONORA_YOUTUBE_BASE (e2e/serve.mjs).
 */

import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The playlist the saved page is for. */
export const E2E_PLAYLIST = 'PLe2eLateNight';

const PRIVATE = '<!DOCTYPE html><html><body><script>var ytInitialData = {"alerts":[{"alertWithButtonRenderer":{"type":"ERROR","text":{"simpleText":"This playlist is private."}}}]};</script></body></html>';

export interface FakeYoutube {
  /** The playlist ids asked for, in order. */
  requests: string[];
  close: () => Promise<void>;
}

export async function startFakeYoutube(port: number): Promise<FakeYoutube> {
  const page = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'fixtures', 'youtube-playlist.html'), 'utf8');
  const fake: { requests: string[] } = { requests: [] };
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? '/', 'http://fake');
    if (url.pathname === '/playlist') {
      const id = url.searchParams.get('list') ?? '';
      fake.requests.push(id);
      const html = id === E2E_PLAYLIST ? page : PRIVATE;
      res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      res.end(html);
      return;
    }
    res.writeHead(404);
    res.end();
  });
  await new Promise<void>((resolve) => server.listen(port, '127.0.0.1', resolve));
  return Object.assign(fake, { close: () => new Promise<void>((resolve) => server.close(() => resolve())) });
}
