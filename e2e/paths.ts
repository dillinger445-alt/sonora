import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Port of the dev bridge during end-to-end tests (not the development 4317). */
export const BRIDGE_PORT = 4318;

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const DATA = join(ROOT, 'e2e', '.data');
/** Generated audio corpus (see `sonora fixtures`). */
export const MUSIC = join(DATA, 'music');
/** Library database for the run; recreated each time the bridge starts. */
export const LIBRARY = join(DATA, 'library');
/**
 * Where the stand-in slskd answers in the one-button setup test: SONORA's
 * own slskd is set up to listen here (SONORA_SLSKD_TEST_PORT).
 */
export const MANAGED_SLSKD_PORT = 4319;
/** The stand-in "release" SONORA installs in that test (SONORA_SLSKD_TEST_ARCHIVE). */
export const SLSKD_ARCHIVE = join(DATA, 'slskd-test-release.zip');
