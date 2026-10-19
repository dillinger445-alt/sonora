import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildManifest, changelogSection, declaredVersions, versionProblems } from './release-lib.ts';

const ROOT = fileURLToPath(new URL('..', import.meta.url));

test('the manifests agree on one version', () => {
  const v = declaredVersions(ROOT);
  const versions = new Set(Object.values(v));
  assert.equal(versions.size, 1, JSON.stringify(v));
  const [only] = [...versions];
  assert.match(only ?? '', /^\d+\.\d+\.\d+/);
  assert.deepEqual(versionProblems(`v${only}`, v), []);
});

test('a tag must name the version the manifests declare', () => {
  const v = { 'package.json': '0.2.0', 'src-tauri/Cargo.toml': '0.2.0', 'src-tauri/tauri.conf.json': '0.2.0' };
  assert.deepEqual(versionProblems('v0.2.0', v), []);
  assert.equal(versionProblems('v0.3.0', v).length, 3);
  assert.match(versionProblems('0.2.0', v)[0] ?? '', /not v<major>/);
  assert.match(versionProblems('v0.2.0', { ...v, 'src-tauri/Cargo.toml': '0.1.0' })[0] ?? '', /Cargo\.toml says 0\.1\.0/);
});

test('release notes come from the matching changelog section', () => {
  const log = '# Changelog\n\n## 0.3.0 – 2026-11-01\n\n- Newer\n\n## [0.2.0]\n\n### New\n- Playlists\n\n## 0.1.0\n- First\n';
  assert.equal(changelogSection(log, '0.2.0'), '### New\n- Playlists');
  assert.equal(changelogSection(log, '0.3.0'), '- Newer');
  assert.equal(changelogSection(log, '0.1.0'), '- First');
  assert.equal(changelogSection(log, '0.1'), null);
  assert.equal(changelogSection(log, '9.9.9'), null);
});

const SIGNED = [
  { name: 'SONORA_0.2.0_x64-setup.exe', signature: 'c2ln-nsis\n' },
  { name: 'SONORA_0.2.0_x64_en-US.msi', signature: 'c2ln-msi' },
  { name: 'SONORA_0.2.0_aarch64.app.tar.gz', signature: 'c2ln-arm' },
  { name: 'SONORA_0.2.0_x64.app.tar.gz', signature: 'c2ln-intel' },
];

test('the updater manifest names a signed installer for every platform', () => {
  const m = buildManifest({ version: '0.2.0', tag: 'v0.2.0', repo: 'o/r', notes: 'Notes', signed: SIGNED, date: new Date('2026-10-20T10:00:00Z') });
  assert.equal(m.version, '0.2.0');
  assert.equal(m.pub_date, '2026-10-20T10:00:00.000Z');
  assert.deepEqual(Object.keys(m.platforms).sort(), [
    'darwin-aarch64',
    'darwin-aarch64-app',
    'darwin-x86_64',
    'darwin-x86_64-app',
    'windows-x86_64',
    'windows-x86_64-msi',
    'windows-x86_64-nsis',
  ]);
  assert.deepEqual(m.platforms['windows-x86_64-nsis'], { signature: 'c2ln-nsis', url: 'https://github.com/o/r/releases/download/v0.2.0/SONORA_0.2.0_x64-setup.exe' });
  assert.equal(m.platforms['windows-x86_64']?.url, m.platforms['windows-x86_64-nsis']?.url);
  assert.equal(m.platforms['darwin-aarch64-app']?.signature, 'c2ln-arm');
  assert.equal(m.platforms['darwin-x86_64']?.signature, 'c2ln-intel');
});

test('a missing, doubled or unuploaded installer stops the release', () => {
  const base = { version: '0.2.0', tag: 'v0.2.0', repo: 'o/r', notes: '' };
  assert.throws(() => buildManifest({ ...base, signed: SIGNED.slice(1) }), /No signed file for the Windows installer/);
  assert.throws(() => buildManifest({ ...base, signed: [...SIGNED, { name: 'SONORA_0.2.1_aarch64.app.tar.gz', signature: 'x' }] }), /More than one file/);
  assert.throws(() => buildManifest({ ...base, signed: SIGNED, uploaded: SIGNED.slice(1).map((s) => s.name) }), /was not uploaded/);
  assert.throws(() => buildManifest({ ...base, signed: SIGNED.map((s, i) => (i === 0 ? { ...s, signature: ' ' } : s)) }), /is empty/);
});
