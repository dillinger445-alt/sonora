#!/usr/bin/env node
/**
 * Release helpers for .github/workflows/release.yml (see docs/RELEASING.md).
 *
 *   node scripts/release.ts check <tag>
 *       Fails unless <tag> is v<version> for the version in package.json,
 *       src-tauri/Cargo.toml and src-tauri/tauri.conf.json.
 *   node scripts/release.ts notes <version>
 *       Prints that version's section of CHANGELOG.md.
 *   node scripts/release.ts manifest <dir> <tag> <owner/repo> <notes-file> [<asset-list>]
 *       Prints latest.json for the in-app updater: one entry per platform,
 *       with the installer's download URL and the signature from its .sig
 *       file in <dir>. <asset-list> (one name per line) confirms that every
 *       installer was uploaded.
 *
 * Requires Node 22.18+ (runs TypeScript directly). No dependencies.
 */

import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildManifest, changelogSection, declaredVersions, versionProblems } from './release-lib.ts';

const ROOT = fileURLToPath(new URL('..', import.meta.url));

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

function main(argv: string[]) {
  const [cmd, ...args] = argv;
  if (cmd === 'check') {
    const [tag] = args;
    if (!tag) fail('usage: release.ts check <tag>');
    const problems = versionProblems(tag, declaredVersions(ROOT));
    if (problems.length) fail(`Cannot release ${tag}:\n${problems.join('\n')}`);
    console.log(`${tag} matches the version in every manifest.`);
  } else if (cmd === 'notes') {
    const [version] = args;
    if (!version) fail('usage: release.ts notes <version>');
    const notes = changelogSection(readFileSync(join(ROOT, 'CHANGELOG.md'), 'utf8'), version);
    if (!notes) fail(`CHANGELOG.md has no section for ${version}.`);
    console.log(notes);
  } else if (cmd === 'manifest') {
    const [dir, tag, repo, notesFile, assetList] = args;
    if (!dir || !tag || !repo || !notesFile) fail('usage: release.ts manifest <dir> <tag> <owner/repo> <notes-file> [<asset-list>]');
    const signed = readdirSync(dir)
      .filter((n) => n.endsWith('.sig'))
      .map((n) => ({ name: n.slice(0, -4), signature: readFileSync(join(dir, n), 'utf8') }));
    const uploaded = assetList
      ? readFileSync(assetList, 'utf8')
          .split('\n')
          .map((l) => l.trim())
          .filter(Boolean)
      : undefined;
    try {
      const m = buildManifest({ version: tag.replace(/^v/, ''), tag, repo, notes: readFileSync(notesFile, 'utf8').trim(), signed, uploaded });
      console.log(JSON.stringify(m, null, 2));
    } catch (e) {
      fail(e instanceof Error ? e.message : String(e));
    }
  } else {
    fail('usage: release.ts check|notes|manifest …');
  }
}

main(process.argv.slice(2));
