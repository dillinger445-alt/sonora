/**
 * What the release workflow checks and writes: versions, release notes from
 * CHANGELOG.md, and the updater manifest (latest.json). Used by
 * scripts/release.ts; tested by scripts/release.test.ts.
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/** The version each manifest declares. */
export function declaredVersions(root: string): Record<string, string | null> {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as { version?: string };
  const conf = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8')) as { version?: string };
  const cargo = readFileSync(join(root, 'src-tauri', 'Cargo.toml'), 'utf8');
  const pkgSection = /\[package\][^[]*?\nversion\s*=\s*"([^"]+)"/.exec(cargo);
  return {
    'package.json': pkg.version ?? null,
    'src-tauri/Cargo.toml': pkgSection?.[1] ?? null,
    'src-tauri/tauri.conf.json': conf.version ?? null,
  };
}

/** Problems with releasing `tag` from these versions (none when it may go ahead). */
export function versionProblems(tag: string, versions: Record<string, string | null>): string[] {
  const m = /^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/.exec(tag);
  if (!m) return [`The tag ${tag} is not v<major>.<minor>.<patch>.`];
  const want = m[1];
  return Object.entries(versions)
    .filter(([, v]) => v !== want)
    .map(([file, v]) => `${file} says ${v ?? 'nothing'}, the tag says ${want}.`);
}

/** The text under `## <version>` (or `## [<version>]`) in a changelog, up to the next `## `. */
export function changelogSection(changelog: string, version: string): string | null {
  const lines = changelog.replace(/\r\n?/g, '\n').split('\n');
  const esc = version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const head = new RegExp(`^##\\s+\\[?v?${esc}\\]?(\\s|$)`);
  const start = lines.findIndex((l) => head.test(l));
  if (start < 0) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((l) => /^##\s/.test(l));
  const body = (end < 0 ? rest : rest.slice(0, end)).join('\n').trim();
  return body || null;
}

export interface Signed {
  /** Installer file name, as uploaded. */
  name: string;
  /** Contents of its .sig file. */
  signature: string;
}

/**
 * Updater platform keys for each installer. Tauri looks for
 * `{os}-{arch}-{installer}` first (so an MSI installation updates from the
 * MSI and an NSIS one from the NSIS installer), then `{os}-{arch}`.
 */
const PLATFORMS: { keys: string[]; pattern: RegExp; what: string }[] = [
  { keys: ['windows-x86_64-nsis', 'windows-x86_64'], pattern: /_x64-setup\.exe$/, what: 'the Windows installer (NSIS)' },
  { keys: ['windows-x86_64-msi'], pattern: /_x64_[A-Za-z-]+\.msi$/, what: 'the Windows MSI package' },
  { keys: ['darwin-aarch64-app', 'darwin-aarch64'], pattern: /_aarch64\.app\.tar\.gz$/, what: 'the macOS app for Apple silicon' },
  { keys: ['darwin-x86_64-app', 'darwin-x86_64'], pattern: /_x64\.app\.tar\.gz$/, what: 'the macOS app for Intel' },
];

export interface Manifest {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
}

export function buildManifest(o: { version: string; tag: string; repo: string; notes: string; signed: Signed[]; uploaded?: string[]; date?: Date }): Manifest {
  const platforms: Manifest['platforms'] = {};
  const problems: string[] = [];
  for (const p of PLATFORMS) {
    const found = o.signed.filter((s) => p.pattern.test(s.name));
    if (found.length !== 1) {
      problems.push(found.length ? `More than one file for ${p.what}: ${found.map((f) => f.name).join(', ')}` : `No signed file for ${p.what}.`);
      continue;
    }
    const f = found[0];
    if (!f) continue;
    if (o.uploaded && !o.uploaded.includes(f.name)) problems.push(`${f.name} has a signature but was not uploaded.`);
    if (!f.signature.trim()) problems.push(`The signature of ${f.name} is empty.`);
    const url = `https://github.com/${o.repo}/releases/download/${encodeURIComponent(o.tag)}/${encodeURIComponent(f.name)}`;
    for (const k of p.keys) platforms[k] = { signature: f.signature.trim(), url };
  }
  if (problems.length) throw new Error(problems.join('\n'));
  return { version: o.version, notes: o.notes, pub_date: (o.date ?? new Date()).toISOString(), platforms };
}
