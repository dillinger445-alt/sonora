#!/usr/bin/env node
// Fetches a static FFmpeg build for a Tauri target and stores it where
// Tauri's `bundle.externalBin` expects it:
//
//   src-tauri/binaries/ffmpeg-<target-triple>[.exe]     the program
//   src-tauri/binaries/ffmpeg-<target-triple>.LICENSE   its licence
//   src-tauri/binaries/ffmpeg-LICENSE.txt               notice bundled with the app
//
// Release builds bundle it with `--config src-tauri/tauri.bundle-ffmpeg.json`;
// debug builds (`npm run app:dev`) use it directly when present.
//
// Usage:
//   node scripts/fetch-ffmpeg.mjs [--target <triple>] [--tag <release>]
//                                 [--if-missing] [--force] [--from-system] [--optional]
//
//   --target       Rust target triple. Defaults to TAURI_ENV_TARGET_TRIPLE (set
//                  by the Tauri CLI) or the host triple from `rustc -vV`.
//                  `universal-apple-darwin` fetches both macOS architectures and
//                  merges them with `lipo`.
//   --tag          eugeneware/ffmpeg-static release tag. Default: b6.1.1, or
//                  SONORA_FFMPEG_TAG / FFMPEG_BINARY_RELEASE. The FFmpeg version
//                  inside differs per platform; the script prints it.
//   --if-missing   Do nothing when a working binary is already in place.
//   --force        Replace the binary even with --if-missing.
//   --from-system  Copy the FFmpeg installed on this machine (SONORA_FFMPEG,
//                  PATH, Homebrew, winget, Scoop, Chocolatey) instead of
//                  downloading. Fine for local builds; package-manager builds
//                  are usually dynamically linked and must not be shipped.
//   --optional     Warn instead of failing when no FFmpeg can be provided. For
//                  development, where SONORA can also use an installed FFmpeg.
//
// Requires Node 22+. No dependencies. Behind an HTTP proxy, run with
// NODE_USE_ENV_PROXY=1 (current Node 22 and 24 releases) so that downloads
// honour HTTPS_PROXY.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, delimiter, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const BIN_DIR = join(ROOT, 'src-tauri', 'binaries');
const NOTICE = join(BIN_DIR, 'ffmpeg-LICENSE.txt');
const DEFAULT_TAG = 'b6.1.1';
const DOWNLOAD_URL = 'https://github.com/eugeneware/ffmpeg-static/releases/download';
const RELEASE_URL = 'https://github.com/eugeneware/ffmpeg-static/releases/tag';
const UNIVERSAL = 'universal-apple-darwin';

/** Rust target triple → ffmpeg-static asset suffix (`<platform>-<arch>`). */
const ASSETS = {
  'aarch64-apple-darwin': 'darwin-arm64',
  'x86_64-apple-darwin': 'darwin-x64',
  'x86_64-pc-windows-msvc': 'win32-x64',
  'x86_64-pc-windows-gnu': 'win32-x64',
  // No Windows ARM64 build is published; Windows 11 on Arm runs the x64 build under emulation.
  'aarch64-pc-windows-msvc': 'win32-x64',
  'x86_64-unknown-linux-gnu': 'linux-x64',
  'aarch64-unknown-linux-gnu': 'linux-arm64',
  'i686-unknown-linux-gnu': 'linux-ia32',
  'armv7-unknown-linux-gnueabihf': 'linux-arm',
};

const tty = process.stdout.isTTY === true;
const color = tty && !process.env.NO_COLOR;
const paint = (code, s) => (color ? `\x1b[${code}m${s}\x1b[0m` : s);
const info = (msg) => console.log(`${paint('36', 'ffmpeg')} ${msg}`);
const warn = (msg) => console.warn(`${paint('33', 'ffmpeg warning')} ${msg}`);
const rel = (p) => relative(ROOT, p) || p;

/** An error whose `kind` decides which advice is printed with it. */
class Failure extends Error {
  /** @param {'usage'|'unsupported'|'missing'|'network'|'no-system'|'other'} kind */
  constructor(kind, message) {
    super(message);
    this.kind = kind;
  }
}

function parseArgs(argv) {
  const args = { target: undefined, tag: undefined, ifMissing: false, force: false, fromSystem: false, optional: false, help: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      const v = argv[++i];
      if (!v || v.startsWith('--')) throw new Failure('usage', `${a} needs a value`);
      return v;
    };
    if (a === '--target') args.target = value();
    else if (a.startsWith('--target=')) args.target = a.slice('--target='.length);
    else if (a === '--tag') args.tag = value();
    else if (a.startsWith('--tag=')) args.tag = a.slice('--tag='.length);
    else if (a === '--if-missing') args.ifMissing = true;
    else if (a === '--force') args.force = true;
    else if (a === '--from-system') args.fromSystem = true;
    else if (a === '--optional') args.optional = true;
    else if (a === '--help' || a === '-h') args.help = true;
    else throw new Failure('usage', `unknown argument: ${a}`);
  }
  return args;
}

function hostTriple() {
  const r = spawnSync('rustc', ['-vV'], { encoding: 'utf8' });
  const m = r.status === 0 ? /^host:\s*(\S+)/m.exec(r.stdout) : null;
  if (m) return m[1];
  // rustc is not on PATH: derive the triple from Node's view of the machine.
  const arch = { x64: 'x86_64', arm64: 'aarch64', ia32: 'i686' }[process.arch] ?? process.arch;
  if (process.platform === 'darwin') return `${arch}-apple-darwin`;
  if (process.platform === 'win32') return `${arch}-pc-windows-msvc`;
  return `${arch}-unknown-linux-gnu`;
}

const exeSuffix = (triple) => (triple.includes('windows') ? '.exe' : '');
const outputPath = (triple) => join(BIN_DIR, `ffmpeg-${triple}${exeSuffix(triple)}`);
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
const mib = (bytes) => `${(bytes / 1_048_576).toFixed(1)} MiB`;

/** Node-style platform and architecture of the binary stored for `triple`. */
function binaryPlatform(triple) {
  if (triple === UNIVERSAL) return { os: 'darwin', arch: process.arch };
  const asset = ASSETS[triple];
  if (asset) {
    const [os, arch] = asset.split('-');
    return { os, arch };
  }
  const os = triple.includes('apple-darwin') ? 'darwin' : triple.includes('windows') ? 'win32' : triple.includes('linux') ? 'linux' : 'other';
  const head = triple.split('-')[0];
  return { os, arch: { x86_64: 'x64', aarch64: 'arm64', i686: 'ia32' }[head] ?? head };
}

/**
 * Whether the binary for `triple` runs on this machine: 'yes' (natively),
 * 'maybe' (only through an emulator that may be absent, such as Rosetta 2)
 * or 'no'.
 */
function runsHere(triple) {
  const { os, arch } = binaryPlatform(triple);
  if (os !== process.platform) return 'no';
  if (arch === process.arch) return 'yes';
  if (process.arch === 'arm64' && arch === 'x64' && (os === 'darwin' || os === 'win32')) return 'maybe';
  if (process.arch === 'x64' && arch === 'ia32') return os === 'win32' ? 'yes' : 'maybe';
  return 'no';
}

/** Runs `file` with `args`; returns its standard output, or null when it fails. */
function output(file, args) {
  const r = spawnSync(file, args, { encoding: 'utf8', timeout: 20_000, windowsHide: true });
  if (r.error || r.status !== 0) return null;
  return r.stdout;
}

/** First line of `<file> -version` (what SONORA itself checks), or null. */
const versionOf = (file) => output(file, ['-version'])?.split(/\r?\n/)[0]?.trim() || null;

function describe(e) {
  if (e?.name === 'TimeoutError') return 'timed out';
  const code = e?.cause?.code;
  const known = {
    ENOTFOUND: 'host name could not be resolved',
    EAI_AGAIN: 'host name could not be resolved',
    ECONNREFUSED: 'connection refused',
    ECONNRESET: 'connection reset',
    UND_ERR_CONNECT_TIMEOUT: 'connection timed out',
  };
  return (code && known[code] ? `${known[code]} (${code})` : null) ?? e?.cause?.message ?? e?.message ?? String(e);
}

/** Downloads `url` into memory, showing real byte progress on a terminal. */
async function download(url, label) {
  let res;
  try {
    res = await fetch(url, {
      redirect: 'follow',
      headers: { 'user-agent': 'sonora-fetch-ffmpeg' },
      signal: AbortSignal.timeout(15 * 60_000),
    });
  } catch (e) {
    throw new Failure('network', `could not reach ${new URL(url).host}: ${describe(e)}`);
  }
  if (res.status === 404) throw new Failure('missing', `not found: ${url}`);
  if (!res.ok) throw new Failure('network', `HTTP ${res.status} ${res.statusText} for ${url}`);
  const total = Number(res.headers.get('content-length')) || 0;
  const chunks = [];
  let received = 0;
  let shown = 0;
  try {
    for await (const chunk of res.body) {
      chunks.push(chunk);
      received += chunk.length;
      if (label && tty && Date.now() - shown > 200) {
        shown = Date.now();
        process.stdout.write(`\r${paint('36', 'ffmpeg')} ${label}: ${mib(received)}${total ? ` of ${mib(total)}` : ''}\x1b[K`);
      }
    }
  } catch (e) {
    throw new Failure('network', `download of ${url} was interrupted: ${describe(e)}`);
  } finally {
    if (shown) process.stdout.write('\r\x1b[K');
  }
  if (total && received !== total) throw new Failure('network', `download of ${url} was incomplete (${received} of ${total} bytes)`);
  return Buffer.concat(chunks);
}

const MAGIC = {
  darwin: [[0xcf, 0xfa, 0xed, 0xfe], [0xca, 0xfe, 0xba, 0xbe]], // Mach-O 64-bit, universal
  win32: [[0x4d, 0x5a]], // MZ
  linux: [[0x7f, 0x45, 0x4c, 0x46]], // ELF
};
const looksExecutable = (buf, os) => (MAGIC[os] ?? []).some((m) => m.every((b, i) => buf[i] === b));

function writeAtomic(file, data, mode) {
  const tmp = `${file}.part`;
  writeFileSync(tmp, data, { mode });
  renameSync(tmp, file);
  chmodSync(file, mode);
}

async function fetchOne(triple, tag) {
  const asset = ASSETS[triple];
  if (!asset) {
    throw new Failure('unsupported', `no prebuilt FFmpeg is published for ${triple} (available: ${[...Object.keys(ASSETS), UNIVERSAL].join(', ')})`);
  }
  const base = `${DOWNLOAD_URL}/${tag}`;
  info(`downloading ffmpeg-${asset}.gz from ffmpeg-static ${tag} for ${triple}`);
  const gz = await download(`${base}/ffmpeg-${asset}.gz`, `ffmpeg-${asset}.gz`);
  let binary;
  try {
    binary = gunzipSync(gz);
  } catch (e) {
    throw new Failure('other', `ffmpeg-${asset}.gz is not a valid gzip file (${e.message})`);
  }
  if (!looksExecutable(binary, binaryPlatform(triple).os)) {
    throw new Failure('other', `ffmpeg-${asset}.gz does not contain a program for ${triple}`);
  }
  // The licence has to travel with the binary, so a missing licence is fatal.
  const license = (await download(`${base}/${asset}.LICENSE`)).toString('utf8');
  const notes = await download(`${base}/${asset}.README`).then(
    (b) => b.toString('utf8'),
    () => null,
  );
  const file = outputPath(triple);
  writeAtomic(file, binary, 0o755);
  writeFileSync(`${file}.LICENSE`, license);
  return { triple, file, origin: `${RELEASE_URL}/${tag} (asset ffmpeg-${asset}.gz)`, license, notes };
}

/** Licence and build facts of a binary that is already on disk. */
function localEntry(triple, file, origin) {
  const runs = runsHere(triple) !== 'no';
  const saved = existsSync(`${file}.LICENSE`) ? readFileSync(`${file}.LICENSE`, 'utf8') : null;
  return {
    triple,
    file,
    origin,
    license: saved || (runs && output(file, ['-hide_banner', '-L'])) || 'Not recorded: run this FFmpeg with -L to print its licence.',
    notes: (runs && output(file, ['-version'])) || null,
  };
}

/**
 * Checks the binary and prints what was produced. A binary that should run
 * here but does not is an error; one built for another machine is reported
 * without being run.
 */
function verify(entry) {
  const runs = runsHere(entry.triple);
  const version = runs === 'no' ? null : versionOf(entry.file);
  if (version === null && runs === 'yes') throw new Failure('other', `${rel(entry.file)} was written but does not run on this machine`);
  if (version === null && runs === 'maybe') warn(`${rel(entry.file)} needs x64 emulation (Rosetta 2 on macOS) to run here, so it was not test-run`);
  const digest = sha256(entry.file);
  info(`${paint('32', 'ready')} ${rel(entry.file)} (${mib(statSync(entry.file).size)})`);
  info(`  ${version ?? `not run on this machine (built for ${entry.triple})`}`);
  info(`  sha256 ${digest}`);
  return { ...entry, version, sha256: digest };
}

/** Drops surrounding blank lines but keeps the indentation of the first line. */
const tidy = (text) => text.replace(/^(\s*\n)+/, '').trimEnd();

/** Writes the notice that release bundles ship as `licenses/FFmpeg.txt`. */
function writeNotice(entries) {
  const rule = '-'.repeat(78);
  const lines = [
    'FFmpeg',
    '======',
    '',
    'SONORA runs FFmpeg as a separate program, which is not linked into SONORA,',
    'to decode audio formats that SONORA does not decode itself. FFmpeg is made by',
    'the FFmpeg developers (https://ffmpeg.org). Its source code is available from',
    'https://ffmpeg.org/download.html and https://github.com/FFmpeg/FFmpeg; the',
    'build notes below identify the version and the libraries in this build.',
    '',
  ];
  for (const e of entries) {
    lines.push(`Program:  ffmpeg for ${e.triple}`);
    lines.push(`Version:  ${e.version ?? 'not determined (fetched on a different platform)'}`);
    lines.push(`Origin:   ${e.origin}`);
    lines.push(`SHA-256:  ${e.sha256}`, '');
  }
  for (const e of entries) {
    if (!e.license) continue;
    lines.push(rule, `Licence of the ${e.triple} build`, rule, '', tidy(e.license), '');
    if (e.notes) lines.push(rule, `Build notes of the ${e.triple} build`, rule, '', tidy(e.notes), '');
  }
  writeFileSync(NOTICE, `${lines.join('\n')}\n`);
  info(`licence notice ${rel(NOTICE)} (bundled as licenses/FFmpeg.txt)`);
}

/** Package-manager shims are small launchers that break when copied; returns the real program. */
function resolveShim(file) {
  if (process.platform !== 'win32') return file;
  const scoopShim = file.replace(/\.exe$/i, '.shim');
  if (existsSync(scoopShim)) {
    const m = /^\s*path\s*=\s*"?([^"\r\n]+?)"?\s*$/m.exec(readFileSync(scoopShim, 'utf8'));
    return m ? m[1] : null;
  }
  const choco = process.env.ChocolateyInstall ?? 'C:\\ProgramData\\chocolatey';
  if (dirname(file).toLowerCase() === join(choco, 'bin').toLowerCase()) {
    const real = join(choco, 'lib', 'ffmpeg', 'tools', 'ffmpeg', 'bin', basename(file));
    return existsSync(real) ? real : null;
  }
  return file;
}

function findSystemFfmpeg() {
  const exe = process.platform === 'win32' ? 'ffmpeg.exe' : 'ffmpeg';
  const candidates = [];
  if (process.env.SONORA_FFMPEG) candidates.push(process.env.SONORA_FFMPEG);
  for (const dir of (process.env.PATH ?? '').split(delimiter)) if (dir) candidates.push(join(dir, exe));
  if (process.platform === 'darwin') candidates.push('/opt/homebrew/bin/ffmpeg', '/usr/local/bin/ffmpeg', '/opt/local/bin/ffmpeg');
  if (process.platform === 'win32') {
    if (process.env.LOCALAPPDATA) candidates.push(join(process.env.LOCALAPPDATA, 'Microsoft', 'WinGet', 'Links', exe));
    if (process.env.USERPROFILE) candidates.push(join(process.env.USERPROFILE, 'scoop', 'shims', exe));
    candidates.push(join(process.env.ChocolateyInstall ?? 'C:\\ProgramData\\chocolatey', 'bin', exe), 'C:\\ffmpeg\\bin\\ffmpeg.exe');
  }
  if (process.platform === 'linux') candidates.push('/usr/bin/ffmpeg', '/usr/local/bin/ffmpeg');
  for (const candidate of candidates) {
    try {
      if (!statSync(candidate).isFile()) continue;
      const real = resolveShim(realpathSync(candidate));
      if (real && versionOf(real) !== null) return real;
    } catch {
      // Not there or not readable: try the next location.
    }
  }
  return null;
}

/** Warns when a copied FFmpeg depends on libraries that other machines will not have. */
function warnIfNotPortable(source, copy) {
  if (process.platform === 'darwin') {
    const r = spawnSync('otool', ['-L', copy], { encoding: 'utf8' });
    const deps = r.status === 0 ? r.stdout.split('\n').slice(1).map((l) => l.trim().split(' ')[0]).filter(Boolean) : [];
    const foreign = deps.filter((d) => !/^(\/usr\/lib\/|\/System\/Library\/|@)/.test(d));
    if (foreign.length) {
      warn(`this FFmpeg loads ${foreign.length} libraries from outside macOS (for example ${foreign[0]}). It works on this Mac, but an app bundle containing it will not run elsewhere. Use the downloaded static build for distribution.`);
    }
  } else if (process.platform === 'linux') {
    const r = spawnSync('ldd', [copy], { encoding: 'utf8' });
    if (r.status === 0 && /libav(codec|format|util)/.test(r.stdout)) {
      warn('this FFmpeg is dynamically linked against the system libav* libraries; do not ship it to other machines.');
    }
  } else if (process.platform === 'win32') {
    const dlls = readdirSync(dirname(source)).filter((f) => /^(avcodec|avformat|avutil|swresample)-\d+\.dll$/i.test(f));
    if (dlls.length) {
      warn(`this FFmpeg is a shared build that needs ${dlls.join(', ')} from ${dirname(source)}; it will not run once copied. Use the downloaded static build.`);
    }
  }
}

function advice(kind, triple) {
  const place = triple ? rel(outputPath(triple)) : 'src-tauri/binaries/ffmpeg-<target-triple>[.exe]';
  const fromSystem = `node scripts/fetch-ffmpeg.mjs --from-system${triple ? ` --target ${triple}` : ''}`;
  const dev = [
    '  • Development needs no bundled FFmpeg: SONORA also finds one through SONORA_FFMPEG, PATH and',
    '    Homebrew. WAV, AIFF and FLAC are decoded without FFmpeg.',
  ];
  switch (kind) {
    case 'usage':
      return ['Run `node scripts/fetch-ffmpeg.mjs --help` for usage.'];
    case 'unsupported':
      return [
        'How to continue:',
        `  • Build on a supported target, or put a self-contained FFmpeg for ${triple} at ${place}`,
        '    together with its licence in src-tauri/binaries/ffmpeg-LICENSE.txt.',
        ...dev,
      ];
    case 'missing':
      return [
        'How to continue:',
        '  • Check the release tag (--tag, SONORA_FFMPEG_TAG); the default is ' + DEFAULT_TAG + '.',
        `  • Or use the FFmpeg installed on this machine: ${fromSystem}`,
      ];
    case 'no-system':
      return [
        'How to continue:',
        '  • Install FFmpeg (macOS: brew install ffmpeg · Windows: winget install Gyan.FFmpeg · Linux: your package manager),',
        '    or set SONORA_FFMPEG to the full path of an ffmpeg program, then run this again.',
        '  • Or download the static build: node scripts/fetch-ffmpeg.mjs',
      ];
    case 'network':
      return [
        'How to continue:',
        '  • Check your connection and try again. Behind a proxy, set NODE_USE_ENV_PROXY=1 (and HTTPS_PROXY).',
        `  • Or use the FFmpeg installed on this machine: ${fromSystem}`,
        '    (macOS: brew install ffmpeg · Windows: winget install Gyan.FFmpeg). Fine for local builds; read the warning it prints.',
        `  • Or put a self-contained FFmpeg at ${place} yourself.`,
        ...dev,
      ];
    default:
      return ['How to continue:', '  • Run again with --force to start from a clean download.', `  • Or use the FFmpeg installed on this machine: ${fromSystem}`, ...dev];
  }
}

async function main(args) {
  if (args.help) {
    const lines = readFileSync(fileURLToPath(import.meta.url), 'utf8').split(/\r?\n/).slice(1);
    const header = lines.slice(0, lines.findIndex((l) => !l.startsWith('//')));
    console.log(header.map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
    return;
  }
  const triple = args.target ?? process.env.TAURI_ENV_TARGET_TRIPLE ?? hostTriple();
  const tag = args.tag ?? process.env.SONORA_FFMPEG_TAG ?? process.env.FFMPEG_BINARY_RELEASE ?? DEFAULT_TAG;
  const out = outputPath(triple);
  mkdirSync(BIN_DIR, { recursive: true });

  if (args.ifMissing && !args.force) {
    // A universal build also compiles each architecture separately, and
    // tauri-build looks for the per-architecture binaries then.
    const needed = triple === UNIVERSAL ? [triple, 'aarch64-apple-darwin', 'x86_64-apple-darwin'] : [triple];
    const usable = (t) => existsSync(outputPath(t)) && (runsHere(t) !== 'yes' || versionOf(outputPath(t)) !== null);
    if (needed.every(usable)) {
      info(`already present: ${rel(out)}`);
      if (!existsSync(NOTICE) && triple !== UNIVERSAL && runsHere(triple) === 'yes') {
        writeNotice([verify(localEntry(triple, out, `already present in ${rel(BIN_DIR)}`))]);
      }
      return;
    }
    if (existsSync(out)) warn(`${rel(out)} is incomplete or does not run; replacing it`);
  }

  if (args.fromSystem) {
    if (triple === UNIVERSAL || runsHere(triple) !== 'yes') {
      throw new Failure('usage', `--from-system can only provide FFmpeg for this machine's own target, not ${triple}`);
    }
    const found = findSystemFfmpeg();
    if (!found) throw new Failure('no-system', 'no working FFmpeg was found on this machine (checked SONORA_FFMPEG, PATH and the usual install locations)');
    info(`copying ${found}`);
    copyFileSync(found, `${out}.part`);
    renameSync(`${out}.part`, out);
    chmodSync(out, 0o755);
    rmSync(`${out}.LICENSE`, { force: true }); // belonged to the previous binary
    warnIfNotPortable(found, out);
    const entry = verify(localEntry(triple, out, `copied from ${found} on the build machine`));
    writeFileSync(`${out}.LICENSE`, entry.license);
    writeNotice([entry]);
    return;
  }

  if (triple === UNIVERSAL) {
    // Tauri compiles each architecture with its own sidecar, then bundles
    // the merged one.
    if (process.platform !== 'darwin') throw new Failure('usage', 'a universal binary can only be created on macOS (it needs `lipo`)');
    const arm = await fetchOne('aarch64-apple-darwin', tag);
    const x64 = await fetchOne('x86_64-apple-darwin', tag);
    rmSync(out, { force: true });
    const r = spawnSync('lipo', ['-create', '-output', out, arm.file, x64.file], { encoding: 'utf8' });
    if (r.status !== 0) throw new Failure('other', `lipo failed: ${r.stderr || r.error?.message}`);
    chmodSync(out, 0o755);
    writeFileSync(`${out}.LICENSE`, `${tidy(arm.license)}\n`);
    const entries = [verify(arm), verify(x64)];
    verify({ triple, file: out });
    writeNotice(entries);
    return;
  }

  writeNotice([verify(await fetchOne(triple, tag))]);
}

let parsed;
try {
  parsed = parseArgs(process.argv.slice(2));
  await main(parsed);
} catch (e) {
  const triple = parsed ? (parsed.target ?? process.env.TAURI_ENV_TARGET_TRIPLE ?? hostTriple()) : undefined;
  const kind = e instanceof Failure ? e.kind : 'other';
  if (parsed?.optional && kind !== 'usage') {
    warn(`${e?.message ?? e}`);
    warn('continuing without a bundled FFmpeg (--optional); SONORA will use one installed on this machine if there is one');
  } else {
    console.error(`${paint('31', 'ffmpeg error')} ${e?.message ?? e}`);
    if (!(e instanceof Failure)) console.error(e?.stack ?? '');
    console.error(['', ...advice(kind, triple)].join('\n'));
    process.exitCode = 1;
  }
}
