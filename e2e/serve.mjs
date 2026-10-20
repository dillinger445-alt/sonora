#!/usr/bin/env node
// Starts the SONORA dev bridge for end-to-end tests:
//  1. builds the bridge and CLI in release mode (analysis in debug is slow),
//  2. generates the audio test corpus once (e2e/.data/music),
//  3. starts from an empty library (e2e/.data/library) and serves dist/.
//
//   node e2e/serve.mjs [port]

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, rmSync } from 'node:fs';
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

// "Move to Trash" uses the freedesktop trash on Linux: keep it inside e2e/.data.
const env = { ...process.env, XDG_DATA_HOME: join(DATA, 'xdg') };
const bridge = spawn(exe('sonora-devserver'), ['--port', port, '--static', join(ROOT, 'dist'), '--data', LIBRARY], { cwd: ROOT, stdio: 'inherit', env });
bridge.on('exit', (code) => process.exit(code ?? 0));
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => bridge.kill(sig));
