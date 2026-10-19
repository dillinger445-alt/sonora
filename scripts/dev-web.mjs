#!/usr/bin/env node
// Browser development setup: runs the SONORA dev bridge (the Rust core served
// over HTTP on 127.0.0.1:4317) and the Vite dev server together, with each
// line of output prefixed by its source. Ctrl+C stops both, and both stop as
// soon as either one exits.
//
//   npm run dev:web                        interface on http://localhost:1420
//   npm run dev:web -- --data <folder>     extra options go to the bridge
//
// The bridge is built in release mode first: analysis in a debug build is too
// slow to be useful. Requires Node 22+ and a Rust toolchain.

import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const BRIDGE_PORT = '4317'; // vite.config.ts proxies /api to this port

if (process.argv.includes('--help') || process.argv.includes('-h')) {
  const lines = readFileSync(fileURLToPath(import.meta.url), 'utf8').split(/\r?\n/).slice(1);
  console.log(lines.slice(0, lines.findIndex((l) => !l.startsWith('//'))).map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
  process.exit(0);
}

const color = process.stdout.isTTY === true && !process.env.NO_COLOR;
const label = (name, code) => (color ? `\x1b[${code}m${name.padEnd(7)}\x1b[0m│` : `${name.padEnd(7)}|`);
const PREFIX = { bridge: label('bridge', '35'), vite: label('vite', '36'), dev: label('dev-web', '90') };
// Children write to pipes, not a terminal; ask them to keep their colours.
const childEnv = { ...process.env, ...(color ? { FORCE_COLOR: '1', CARGO_TERM_COLOR: 'always' } : {}) };

const say = (msg) => process.stdout.write(`${PREFIX.dev} ${msg}\n`);
const children = new Set();
let stopping = false;

/** Copies `stream` line by line to `out` with a prefix; `consume` may swallow lines. */
function relay(stream, out, prefix, consume) {
  createInterface({ input: stream, crlfDelay: Infinity }).on('line', (line) => {
    if (!consume?.(line)) out.write(`${prefix} ${line}\n`);
  });
}

/** Starts a process; resolves with how it ended. */
function start(name, command, args, consume) {
  const child = spawn(command, args, { cwd: ROOT, env: childEnv, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  children.add(child);
  relay(child.stdout, process.stdout, PREFIX[name], consume);
  relay(child.stderr, process.stderr, PREFIX[name]);
  const ended = new Promise((done) => {
    child.once('error', (error) => {
      children.delete(child);
      done({ code: 1, error });
    });
    child.once('exit', (code, signal) => {
      children.delete(child);
      done({ code, signal });
    });
  });
  return { child, ended };
}

/** Ctrl+C reaches the whole process group, so children may report it first. */
const interrupted = ({ code, signal }) => signal === 'SIGINT' || code === 130 || code === 0xc000013a || code === -1073741510;

async function stopAll(exitCode, reason) {
  // A second caller waits for the first one, which ends the process.
  if (stopping) return new Promise(() => {});
  stopping = true;
  if (reason) say(reason);
  for (const child of children) child.kill('SIGTERM');
  const deadline = Date.now() + 5000;
  while (children.size > 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
  for (const child of children) child.kill('SIGKILL');
  process.exit(exitCode);
}

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => stopAll(0, 'stopping…'));
}

/** Builds the bridge and returns the path of its executable. */
async function buildBridge() {
  say('building the dev bridge (cargo build --release -p sonora-devserver)…');
  let executable = null;
  const build = start('bridge', 'cargo', ['build', '--release', '-p', 'sonora-devserver', '--message-format=json-render-diagnostics'], (line) => {
    // Cargo's machine-readable messages on stdout: keep the executable path, print nothing.
    if (!line.startsWith('{')) return false;
    try {
      const msg = JSON.parse(line);
      if (msg.reason === 'compiler-artifact' && msg.target?.name === 'sonora-devserver' && msg.executable) executable = msg.executable;
    } catch {
      return false;
    }
    return true;
  });
  const result = await build.ended;
  if (result.error?.code === 'ENOENT') await stopAll(1, 'cargo was not found. Install Rust from https://rustup.rs and try again.');
  if (result.error) await stopAll(1, `could not run cargo: ${result.error.message}`);
  if (interrupted(result)) await stopAll(0);
  if (result.code !== 0) await stopAll(result.code ?? 1, 'the dev bridge failed to build.');
  if (!executable) await stopAll(1, 'cargo did not report the sonora-devserver executable.');
  return executable;
}

function viteCli() {
  const pkgFile = join(ROOT, 'node_modules', 'vite', 'package.json');
  if (!existsSync(pkgFile)) return null;
  const pkg = JSON.parse(readFileSync(pkgFile, 'utf8'));
  const bin = typeof pkg.bin === 'string' ? pkg.bin : pkg.bin?.vite;
  return bin ? join(dirname(pkgFile), bin) : null;
}

const vite = viteCli();
if (!vite) {
  say('Vite is not installed. Run `npm install` first.');
  process.exit(1);
}

const executable = await buildBridge();
const bridge = start('bridge', executable, ['--port', BRIDGE_PORT, ...process.argv.slice(2)]);
// Run Vite's CLI with this Node directly: no shell, works the same on every OS.
const web = start('vite', process.execPath, [vite]);
say(`interface http://localhost:1420 · bridge http://127.0.0.1:${BRIDGE_PORT} · Ctrl+C stops both`);

const [name, result] = await Promise.race([bridge.ended.then((r) => ['bridge', r]), web.ended.then((r) => ['vite', r])]);
// Give a pending Ctrl+C a moment to be handled as a normal stop.
await new Promise((r) => setTimeout(r, 100));
if (interrupted(result)) await stopAll(0);
const how = result.error ? `could not start (${result.error.message})` : result.signal ? `was stopped by ${result.signal}` : `exited with code ${result.code}`;
await stopAll(result.code || 1, `${name} ${how}; stopping the other process.`);
