#!/usr/bin/env node
// Starts the SONORA dev bridge for end-to-end tests:
//  1. builds the bridge and CLI in release mode (analysis in debug is slow),
//  2. generates the audio test corpus once (e2e/.data/music),
//  3. starts from an empty library (e2e/.data/library) and serves dist/.
//
//   node e2e/serve.mjs [port]

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DATA = join(ROOT, 'e2e', '.data');
const MUSIC = join(DATA, 'music');
const LIBRARY = join(DATA, 'library');
const port = process.argv[2] ?? '4318';
const exe = (name) => join(ROOT, 'target', 'release', process.platform === 'win32' ? `${name}.exe` : name);

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: ROOT, stdio: 'inherit' });
  if (r.error) {
    console.error(`could not run ${cmd}: ${r.error.message}`);
    process.exit(1);
  }
  if (r.status !== 0) process.exit(r.status ?? 1);
}

if (!existsSync(join(ROOT, 'dist', 'index.html'))) {
  console.error('dist/ is missing: run `npm run build` first.');
  process.exit(1);
}
run('cargo', ['build', '--release', '-p', 'sonora-devserver', '-p', 'sonora-cli']);
if (!existsSync(join(MUSIC, 'manifest.json'))) {
  run(exe('sonora'), ['fixtures', MUSIC, '--seconds', '20']);
}
rmSync(LIBRARY, { recursive: true, force: true });

// A stand-in for slskd's release archive: what SONORA's one-button setup
// downloads, checks and unpacks (it "downloads" this file in the tests, and
// launchd is only noted; see TestSystem in slskd_service.rs).
const RELEASE = join(DATA, 'slskd-release');
const ARCHIVE = join(DATA, 'slskd-test-release.zip');
rmSync(ARCHIVE, { force: true });
mkdirSync(join(RELEASE, 'wwwroot'), { recursive: true });
writeFileSync(join(RELEASE, 'slskd'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
writeFileSync(join(RELEASE, 'wwwroot', 'index.html'), '<html></html>');
const zipped = spawnSync('zip', ['-q', '-r', ARCHIVE, '.'], { cwd: RELEASE, stdio: 'inherit' });
if (zipped.status !== 0) console.warn('zip is not available: the one-button setup test cannot run');

// "Move to Trash" uses the freedesktop trash on Linux: keep it inside e2e/.data.
// slskd's API key goes to a file in the library folder, never the system's
// keychain; the Soulseek download folder is looked at every second.
const env = {
  ...process.env,
  XDG_DATA_HOME: join(DATA, 'xdg'),
  SONORA_SECRET_STORE: 'file',
  SONORA_SOURCE_POLL_SECONDS: '1',
  // slskd set up by SONORA: a stand-in system and a stand-in release, with
  // SONORA's slskd answering on the port the test's fake listens on.
  SONORA_SLSKD_SYSTEM: 'test',
  SONORA_SLSKD_TEST_ARCHIVE: ARCHIVE,
  SONORA_SLSKD_TEST_PORT: '4319',
  SONORA_SLSKD_WAIT_SECONDS: '6',
  // YouTube playlists come from a stand-in started by the playlist test.
  SONORA_YOUTUBE_BASE: 'http://127.0.0.1:4320',
  HOME: join(DATA, 'home'),
};
mkdirSync(env.HOME, { recursive: true });
const bridge = spawn(exe('sonora-devserver'), ['--port', port, '--static', join(ROOT, 'dist'), '--data', LIBRARY], { cwd: ROOT, stdio: 'inherit', env });
bridge.on('exit', (code) => process.exit(code ?? 0));
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => bridge.kill(sig));
