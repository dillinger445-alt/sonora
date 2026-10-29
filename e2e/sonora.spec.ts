/**
 * End-to-end tests of the SONORA interface against the real analysis core.
 *
 * The tests run in order on one library: the first imports the generated
 * corpus through the interface, the rest work with the analyzed tracks.
 */

import { expect, test, type Locator, type Page } from '@playwright/test';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { incompletePath, startFakeSlskd } from './fakeSlskd.ts';
import { E2E_PLAYLIST, startFakeYoutube } from './fakeYoutube.ts';
import { DATA, LIBRARY, MANAGED_SLSKD_PORT, MUSIC, YOUTUBE_PORT } from './paths.ts';

test.describe.configure({ mode: 'serial' });

/** Waits until the core reports no scan or indexing activity. */
async function waitIdle(page: Page, timeoutMs = 120_000): Promise<void> {
  await expect
    .poll(
      async () =>
        page.evaluate(async () => {
          const r = await fetch('/api/call', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', 'X-Sonora-Client': 'e2e' },
            body: JSON.stringify({ method: 'scan.status' }),
          });
          const s = (await r.json()) as { result: { state: string; indexing: string | null } };
          return s.result.state === 'idle' && s.result.indexing == null;
        }),
      { timeout: timeoutMs, intervals: [250, 500, 1000] },
    )
    .toBe(true);
}

/** Calls the core API the way the interface does. */
async function api<T>(page: Page, method: string, params: unknown = {}): Promise<T> {
  return page.evaluate(
    async ({ method, params }) => {
      const r = await fetch('/api/call', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Sonora-Client': 'e2e' },
        body: JSON.stringify({ method, params }),
      });
      const j = (await r.json()) as { ok: boolean; result: unknown; error?: { message: string } };
      if (!j.ok) throw new Error(j.error?.message ?? method);
      return j.result;
    },
    { method, params },
  ) as Promise<T>;
}

function rows(page: Page) {
  return page.locator('.tbody .trow:not(.is-placeholder)');
}

/** The distinct texts of one column over the rows shown (once they settle). */
async function columnValues(page: Page, column: string): Promise<string[]> {
  const texts = await page.locator(`.tbody .trow:not(.is-placeholder) .tcell-${column}`).allInnerTexts();
  return [...new Set(texts.map((t) => t.trim()))].sort();
}

function row(page: Page, title: string) {
  return page.locator('.tbody .trow').filter({ has: page.locator('.cell-title', { hasText: new RegExp(`^${title}$`) }) });
}

/** The titles in the open playlist, in order. */
function playlistTitles(page: Page): Promise<string[]> {
  return page.locator('.pt .tbody .trow .cell-title > span.ellipsis').allInnerTexts();
}

/**
 * Drags with the mouse as a person would: press, move a little, move over
 * the target (its middle, or `at` from its top left corner), let go.
 */
async function drag(page: Page, from: Locator, to: Locator, at?: { x?: number; y?: number }): Promise<void> {
  const a = await from.boundingBox();
  const b = await to.boundingBox();
  if (!a || !b) throw new Error('drag: the source or the target is not on screen');
  const x = a.x + a.width / 2;
  const y = a.y + a.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 8, y + 8, { steps: 2 });
  await page.mouse.move(b.x + (at?.x ?? b.width / 2), b.y + (at?.y ?? b.height / 2), { steps: 10 });
  await page.mouse.up();
}

test('starts empty, imports a folder by path and analyzes it in the background', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Add music to analyze' })).toBeVisible();
  await expect(page.getByText('Local only')).toBeVisible();

  // The browser build has no native picker: it asks for a path instead.
  await page.getByRole('button', { name: 'Add folder…' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add folders by path' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('textbox').fill('relative/path');
  await dialog.getByRole('button', { name: 'Add to library' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Use full paths');
  await dialog.getByRole('textbox').fill(MUSIC);
  await dialog.getByRole('button', { name: 'Add to library' }).click();
  await expect(dialog).toBeHidden();

  // Progress is visible while it runs, then the summary.
  await page.getByRole('link', { name: /Import queue/ }).click();
  await expect(page.getByRole('heading', { name: 'Import queue' })).toBeVisible();
  await waitIdle(page);
  await expect(page.getByText('Completed', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Results' })).toBeVisible();
  await expect(page.locator('.results-table tbody tr').first()).toBeVisible();

  // The overview reflects the database.
  await page.getByRole('link', { name: /^Overview/ }).click();
  await expect(page.getByRole('heading', { name: 'Quality profile' })).toBeVisible();
  await expect(page.getByRole('figure', { name: /score distribution of \d+ analyzed tracks/ })).toBeVisible();
  await expect(page.locator('.attn-row', { hasText: 'Clipping' }).locator('.attn-n')).not.toHaveText('0');
});

test('library: search, sort, filter and keyboard navigation', async ({ page }) => {
  await page.goto('/#/library');
  await expect(rows(page).first()).toBeVisible();
  const total = await rows(page).count();
  expect(total).toBeGreaterThan(20);

  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clipped');
  await expect(rows(page)).toHaveCount(2);
  await expect(page.getByText('2 tracks', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Clear search' }).click();
  await expect(rows(page)).toHaveCount(total);

  // Sort by the overall score: worst first after toggling.
  await page.getByRole('columnheader', { name: 'Overall' }).getByRole('button').click();
  await page.getByRole('columnheader', { name: 'Overall' }).getByRole('button').click();
  await expect(page.getByRole('columnheader', { name: 'Overall' })).toHaveAttribute('aria-sort', 'ascending');
  await expect(rows(page).first().locator('.cell-title')).toHaveText('silent');

  // Filter by issue.
  await page.getByRole('button', { name: /^Issues/ }).click();
  await page.getByRole('menuitemcheckbox', { name: /Clipping/ }).click();
  await page.keyboard.press('Escape');
  await expect(rows(page)).not.toHaveCount(total);
  // Not empty either: the loop below checks nothing over no rows (B99).
  await expect(rows(page).first().locator('.chip-red', { hasText: 'Clipping' })).toBeVisible();
  for (const r of await rows(page).all()) await expect(r.locator('.chip-red', { hasText: 'Clipping' })).toBeVisible();
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(rows(page)).toHaveCount(total);

  // By the date tracks were added: all of them came in today; none in 2020.
  const addedFilter = page.getByRole('toolbar', { name: 'Filters' }).getByRole('button', { name: /^Added/ });
  await addedFilter.click();
  await page.getByRole('button', { name: 'Today', exact: true }).click();
  await expect(addedFilter).toContainText('Today');
  await expect(rows(page)).toHaveCount(total);
  await addedFilter.click();
  await page.getByLabel('Added on or after').fill('2020-01-01');
  await page.getByLabel('Added on or before').fill('2020-12-31');
  await page.keyboard.press('Escape');
  await expect(page.getByText('No tracks match these filters.')).toBeVisible();
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(rows(page)).toHaveCount(total);
  // The Added column shows the date and sorts newest first.
  const addedHead = page.getByRole('columnheader', { name: 'Added' });
  await expect(addedHead).toBeVisible();
  await addedHead.getByRole('button').click();
  await expect(addedHead).toHaveAttribute('aria-sort', 'descending');

  // Keyboard: focus the grid, move, open with Enter.
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clean_master_24bit');
  await expect(rows(page)).toHaveCount(1);
  await page.getByRole('grid', { name: 'Music library' }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('complementary', { name: 'Track details' })).toContainText('clean_master_24bit');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/#\/track\/\d+/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('clean_master_24bit');
});

test('Delete acts on the track the details panel shows, and switching scores keeps the selection', async ({ page }) => {
  await page.goto('/#/library');
  const grid = page.getByRole('grid', { name: 'Music library' });
  const panel = page.getByRole('complementary', { name: 'Track details' });
  await expect(rows(page).nth(2)).toBeVisible();
  await rows(page).nth(2).click();
  const third = (await rows(page).nth(2).locator('.cell-title').innerText()).trim();
  await expect(panel.getByRole('heading', { level: 2 })).toHaveText(third);

  // Jump to the last row: the panel, the selection and Delete all follow it.
  await page.keyboard.press('End');
  const lastRow = page.locator('.tbody .trow.is-active');
  await expect(lastRow).toHaveAttribute('aria-selected', 'true');
  const last = (await lastRow.locator('.cell-title').innerText()).trim();
  expect(last).not.toBe(third);
  await expect(panel.getByRole('heading', { level: 2 })).toHaveText(last);
  await page.keyboard.press('Delete');
  const dialog = page.getByRole('dialog', { name: 'Remove from library?' });
  await expect(dialog).toContainText(`“${last}”`);
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();

  // The score shown is a view setting: rows and selection stay.
  await page.getByRole('radio', { name: 'DJ' }).click();
  await expect(lastRow).toHaveAttribute('aria-selected', 'true');
  await expect(panel.getByRole('heading', { level: 2 })).toHaveText(last);
  await page.getByRole('radio', { name: 'Overall' }).click();
  await expect(grid).toBeVisible();
});

test('right-clicking a row opens its menu, for the row or for the selection it is in', async ({ page }) => {
  await page.goto('/#/library');
  const grid = page.getByRole('grid', { name: 'Music library' });
  await expect(rows(page).nth(3)).toBeVisible();
  const first = (await rows(page).nth(0).locator('.cell-title').innerText()).trim();

  // One row: it is selected first, and the menu acts on it.
  await rows(page).nth(0).click({ button: 'right' });
  const menu = page.getByRole('menu', { name: `Actions for ${first}` });
  await expect(menu).toBeVisible();
  await expect(rows(page).nth(0)).toHaveAttribute('aria-selected', 'true');
  await expect(menu.getByRole('menuitem', { name: /^Remove from library…/ })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: /^Move to Trash…/ })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(grid).toBeFocused();

  // Within a selection of several, it acts on all of them.
  await rows(page).nth(1).click({ modifiers: ['Shift'] });
  await rows(page).nth(1).click({ button: 'right' });
  const several = page.getByRole('menu', { name: 'Actions for 2 tracks' });
  await several.getByRole('menuitem', { name: 'Remove 2 tracks from the library…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Remove 2 tracks from the library?' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(rows(page).nth(0)).toHaveAttribute('aria-selected', 'true');

  // From the keyboard: Shift+F10 opens the active row's menu at its first item.
  await grid.focus();
  await page.keyboard.press('Shift+F10');
  const keyboardMenu = page.getByRole('menu', { name: /^Actions for/ });
  await expect(keyboardMenu.getByRole('menuitem').first()).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(grid).toBeFocused();
});

test('track analysis shows scores, real visualizations and explained findings', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clipped_master');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'clipped_master').dblclick();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('clipped_master');

  for (const name of ['Overall', 'DJ', 'Listening']) {
    await expect(page.getByRole('img', { name: new RegExp(`^${name} score: \\d+ out of 100`) })).toBeVisible();
  }
  await expect(page.locator('.verdict-text')).toContainText(/clipping/i);

  // The canvases contain drawn data, not empty frames.
  const lanes = page.getByRole('group', { name: /Waveform, spectrogram and loudness/ });
  await expect(lanes).toBeVisible();
  const painted = await lanes.locator('canvas').evaluateAll((canvases) =>
    canvases.slice(0, 3).map((c) => {
      const el = c as HTMLCanvasElement;
      const d = el.getContext('2d')?.getImageData(0, 0, el.width, el.height).data;
      if (!d) return 0;
      let n = 0;
      for (let i = 3; i < d.length; i += 4 * 16) if ((d[i] ?? 0) > 0) n += 1;
      return n / (d.length / (4 * 16));
    }),
  );
  for (const share of painted) expect(share).toBeGreaterThan(0.05);

  // Findings explain what, why and how reliable; timestamps move the cursor.
  const finding = page.locator('.finding', { hasText: 'Hard clipping detected' });
  await finding.getByRole('button', { expanded: false }).click();
  await expect(finding.getByText('What was measured')).toBeVisible();
  await expect(finding.getByText('Why it matters')).toBeVisible();
  await expect(finding.getByText('How reliable this is')).toBeVisible();
  await expect(finding.getByRole('img', { name: 'Evidence: Confirmed' })).toBeVisible();
  await finding.locator('.time-chip').first().click();
  await expect(page.locator('.lanes-cursor')).toBeVisible();
  await expect(page.locator('.lanes-readout')).toContainText('dBFS');

  await expect(page.getByRole('heading', { name: 'Sub-scores' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Technical details' })).toBeVisible();
  await expect(page.locator('.tech-row', { hasText: 'True peak' })).toContainText('dBTP');
});

test('needs attention groups problems and explains failures', async ({ page }) => {
  await page.goto('/#/attention');
  await page.getByRole('tab', { name: /^Clipping/ }).click();
  await expect(row(page, 'clipped_master')).toBeVisible();
  await page.getByRole('tab', { name: /^Failed/ }).click();
  await expect(rows(page).first()).toBeVisible();
  await expect(page.locator('.tcell-issues', { hasText: /Unsupported format|corrupt|empty/ }).first()).toBeVisible();
  await page.getByRole('tab', { name: /^Lossy source/ }).click();
  await expect(row(page, 'transcode_mp3_128')).toBeVisible();
});

test('duplicates: identical files at once, similar recordings after a fingerprint comparison', async ({ page }) => {
  await page.goto('/#/duplicates');
  await expect(page.getByRole('heading', { name: /^Identical files/ }).first()).toBeVisible();
  // B104: each row's five buttons fit their cell (the last two were cut off).
  const cut = await page.locator('.dup-table td.cell-actions').evaluateAll(
    (cells) => cells.filter((td) => [...td.querySelectorAll('button')].some((b) => b.getBoundingClientRect().right > td.getBoundingClientRect().right + 0.5)).length,
  );
  expect(cut).toBe(0);
  await page.getByRole('button', { name: 'Find similar recordings' }).click();
  await expect(page.getByRole('heading', { name: /^Same recording, different encoding/ }).first()).toBeVisible({ timeout: 60_000 });
  // Comparing is read-only: the side-by-side view shows the metrics.
  const group = page.locator('.dup-group.kind-same_recording').first();
  await group.getByRole('button', { name: 'Compare' }).click();
  await expect(group.locator('.compare-table')).toContainText('Sample rate');
});

test('export writes a CSV of the current view', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clean_master');
  // The master in six containers and copies.
  await expect(rows(page)).toHaveCount(6);
  const n = 6;
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export these tracks as CSV' }).click();
  const download = await downloadPromise;
  const file = await download.path();
  const text = readFileSync(file, 'utf8');
  const lines = text.trim().split(/\r?\n/);
  expect(lines[0]).toContain('title');
  expect(lines.length).toBe(n + 1);
});

test('removing a track asks first and never touches the file', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('dark_master');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'dark_master').click();
  const panel = page.getByRole('complementary', { name: 'Track details' });
  await panel.getByRole('button', { name: 'Remove from library' }).click();
  const dialog = page.getByRole('dialog', { name: 'Remove from library?' });
  await expect(dialog).toContainText('The audio file stays where it is');
  // Cancel is focused first, so Enter does not delete by accident.
  await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(dialog).toBeHidden();
  await expect(row(page, 'dark_master')).toBeVisible();

  await panel.getByRole('button', { name: 'Remove from library' }).click();
  await dialog.getByRole('button', { name: 'Remove from library' }).click();
  await expect(page.getByText('Removed 1 track from the library')).toBeVisible();
  await expect(rows(page)).toHaveCount(0);
  // The keyboard stays in the table rather than falling back to the page.
  await expect(page.getByRole('grid', { name: 'Music library' })).toBeFocused();
  expect(existsSync(join(MUSIC, 'dark_master.flac'))).toBe(true);
});

test('rescanning a folder picks up new files, skips unchanged ones and leaves removed tracks out until brought back', async ({ page }) => {
  const extra = join(MUSIC, 'zz_new_copy.flac');
  rmSync(extra, { force: true });
  copyFileSync(join(MUSIC, 'other_track.flac'), extra);
  try {
    await page.goto('/#/folders');
    await expect(page.getByRole('cell', { name: /music/ }).first()).toBeVisible();
    await page.getByRole('button', { name: 'Rescan this folder' }).first().click();
    await waitIdle(page);
    await page.goto('/#/library');
    const search = page.getByRole('searchbox', { name: 'Search tracks' });
    await search.fill('zz_new_copy');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('.score-num').first()).not.toHaveText('—');
    // The track removed earlier stays out, though its file is still there.
    await search.fill('dark_master');
    await expect(page.getByText('No tracks match')).toBeVisible();
    await expect(rows(page)).toHaveCount(0);
    // Unchanged files are skipped: the scan analyzed only the new file.
    await page.goto('/#/history');
    const latest = page.locator('.history-table tbody tr').first();
    await expect(latest).toContainText('Folder rescan');
    await expect(latest.locator('td').nth(3)).toHaveText('1');

    // The folder says so, and brings it back on request.
    await page.goto('/#/folders');
    const note = page.locator('.folder-removed').first();
    await expect(note).toContainText('1 track removed from the library is left out of rescans');
    await note.getByRole('button', { name: 'Bring back' }).click();
    await expect(page.getByText(/Bringing back 1 track/)).toBeVisible();
    await waitIdle(page);
    await expect(page.locator('.folder-removed')).toHaveCount(0);
    await page.goto('/#/library');
    await page.getByRole('searchbox', { name: 'Search tracks' }).fill('dark_master');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('.score-num').first()).not.toHaveText('—');
  } finally {
    rmSync(extra, { force: true });
  }
});

test('Move to Trash puts the audio file in the Trash and takes its track out of the library', async ({ page }) => {
  // Elsewhere the system's own Trash would be used: this checks the one kept in e2e/.data.
  test.skip(process.platform !== 'linux', 'uses the freedesktop trash');
  const name = 'zz_trash_me.flac';
  const file = join(MUSIC, name);
  const trash = join(DATA, 'xdg', 'Trash');
  rmSync(join(trash, 'files', name), { force: true });
  rmSync(join(trash, 'info', `${name}.trashinfo`), { force: true });
  copyFileSync(join(MUSIC, 'other_track.flac'), file);
  try {
    // Added by itself, by path (the browser build has no file picker).
    await page.goto('/#/library');
    await page.getByRole('button', { name: 'Import music' }).click();
    await page.getByRole('menuitem', { name: /Add files…/ }).click();
    const add = page.getByRole('dialog', { name: 'Add files or folders by path' });
    await add.getByRole('textbox').fill(file);
    await add.getByRole('button', { name: 'Add to library' }).click();
    await expect(add).toBeHidden();
    await waitIdle(page);
    await page.getByRole('searchbox', { name: 'Search tracks' }).fill('zz_trash_me');
    await expect(rows(page)).toHaveCount(1);
    await row(page, 'zz_trash_me').click();

    const panel = page.getByRole('complementary', { name: 'Track details' });
    await panel.getByRole('button', { name: 'Move to Trash…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Move the file to the Trash?' });
    await expect(dialog).toContainText('You can put it back from the Trash');
    // Cancel is first: Enter does not throw a file away by accident.
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await dialog.getByRole('button', { name: 'Move to Trash' }).click();
    await expect(page.getByText('Moved 1 file to the Trash')).toBeVisible();
    await expect(rows(page)).toHaveCount(0);

    // The file is in the Trash, with a note of where it came from; nothing else moved.
    expect(existsSync(file)).toBe(false);
    expect(existsSync(join(trash, 'files', name))).toBe(true);
    expect(readFileSync(join(trash, 'info', `${name}.trashinfo`), 'utf8')).toContain(`Path=${file.split('/').map(encodeURIComponent).join('/')}`);
    expect(existsSync(join(MUSIC, 'other_track.flac'))).toBe(true);
  } finally {
    rmSync(file, { force: true });
    rmSync(join(trash, 'files', name), { force: true });
    rmSync(join(trash, 'info', `${name}.trashinfo`), { force: true });
  }
});

test('settings persist and the default score applies', async ({ page }) => {
  await page.goto('/#/settings');
  const group = page.getByRole('radiogroup', { name: 'Default score' });
  await group.getByRole('radio', { name: 'DJ' }).click();
  await expect(group.getByRole('radio', { name: 'DJ' })).toHaveAttribute('aria-checked', 'true');
  await page.reload();
  await expect(page.getByRole('radiogroup', { name: 'Default score' }).getByRole('radio', { name: 'DJ' })).toHaveAttribute('aria-checked', 'true');
  await page.goto('/#/library');
  await expect(page.getByRole('radiogroup', { name: 'Score shown' }).getByRole('radio', { name: 'DJ' })).toHaveAttribute('aria-checked', 'true');
  // Restore.
  await page.goto('/#/settings');
  await page.getByRole('radiogroup', { name: 'Default score' }).getByRole('radio', { name: 'Overall' }).click();
  await expect(page.getByText('SONORA works on this computer')).toBeVisible();
});

test('history lists scans with their results', async ({ page }) => {
  await page.goto('/#/history');
  await expect(page.locator('.history-table tbody tr').first()).toBeVisible();
  await page.locator('.history-table tbody tr').last().click();
  await expect(page).toHaveURL(/#\/history\/\d+/);
  await expect(page.getByRole('heading', { name: 'Results' })).toBeVisible();
  await expect(page.locator('.results-table tbody tr').first()).toBeVisible();
});

test('menus work from the keyboard and return focus', async ({ page }) => {
  await page.goto('/#/library');
  await expect(rows(page).first()).toBeVisible();
  const trigger = page.getByRole('toolbar', { name: 'Filters' }).getByRole('button', { name: /^Format/ });
  await trigger.focus();
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu', { name: 'Format filter' });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole('menuitemcheckbox').first()).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press(' ');
  await expect(menu.getByRole('menuitemcheckbox', { name: /Any lossy/ })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(trigger).toBeFocused();
  await expect(rows(page).first().locator('.fmt-name')).toHaveText(/MP3|AAC|Vorbis|Opus/);
  for (const r of await rows(page).all()) await expect(r.locator('.fmt-name')).toHaveText(/MP3|AAC|Vorbis|Opus/);

  // Tab leaves an open menu for the control after its trigger.
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByRole('menuitemcheckbox').first()).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(menu).toBeHidden();
  await expect(page.getByRole('toolbar', { name: 'Filters' }).getByRole('button', { name: /^Issues/ })).toBeFocused();
  // Shift+Tab from an open menu goes back to its trigger.
  await page.keyboard.press('Shift+Tab');
  await expect(trigger).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByRole('menuitemcheckbox').first()).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(menu).toBeHidden();
  await expect(trigger).toBeFocused();
});

test('category tabs follow the tabs pattern', async ({ page }) => {
  await page.goto('/#/attention');
  const all = page.getByRole('tab', { name: /^All/ });
  await expect(all).toHaveAttribute('aria-selected', 'true');
  const panel = page.getByRole('tabpanel');
  await expect(panel).toHaveAttribute('aria-labelledby', (await all.getAttribute('id')) ?? 'missing');
  // Only the selected tab is in the Tab order; arrows move and select.
  await expect(page.getByRole('tab', { name: /^Clipping/ })).toHaveAttribute('tabindex', '-1');
  await all.focus();
  await page.keyboard.press('ArrowRight');
  const clipping = page.getByRole('tab', { name: /^Clipping/ });
  await expect(clipping).toBeFocused();
  await expect(clipping).toHaveAttribute('aria-selected', 'true');
  await expect(page).toHaveURL(/#\/attention\/clipping|#\/attention\?flag=clipping/);
  await page.keyboard.press('Home');
  await expect(all).toBeFocused();
});

// ------------------------------------------------------------------- DJ

test('tempo and key: detected, filtered by range and on the Camelot wheel, shown in any notation', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first().locator('.tcell-bpm')).toHaveText('128');
  await expect(rows(page).first().locator('.tcell-key')).toHaveText('8A');
  await page.getByRole('button', { name: 'Clear search' }).click();
  await expect.poll(() => rows(page).count()).toBeGreaterThan(20);
  const total = await rows(page).count();
  const toolbar = page.getByRole('toolbar', { name: 'Filters' });

  // A tempo range, then the same range at half and double time.
  const bpmButton = toolbar.getByRole('button', { name: /^BPM/ });
  await bpmButton.click();
  const tempo = page.getByRole('dialog', { name: 'Tempo filter' });
  await tempo.getByRole('textbox', { name: 'Lowest BPM' }).fill('126');
  await tempo.getByRole('textbox', { name: 'Highest BPM' }).fill('130');
  // other_track, and its copy from the rescan test.
  await expect.poll(() => columnValues(page, 'bpm')).toEqual(['128']);
  await expect(row(page, 'other_track')).toBeVisible();
  await tempo.getByRole('textbox', { name: 'Lowest BPM' }).fill('60');
  await tempo.getByRole('textbox', { name: 'Highest BPM' }).fill('64');
  await expect(page.getByText('No tracks match these filters.')).toBeVisible();
  await tempo.getByRole('checkbox', { name: /half and double/ }).check();
  // 120–128 BPM: the 124 BPM master and its copies, and the 128 BPM track.
  await expect.poll(async () => (await columnValues(page, 'bpm')).every((v) => /^12[48]/.test(v)) && (await rows(page).count()) > 2).toBe(true);
  await page.keyboard.press('Escape');
  await expect(tempo).toBeHidden();
  await expect(bpmButton).toBeFocused();
  await expect(bpmButton).toContainText('60–64');
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(rows(page)).toHaveCount(total);

  // Keys on the wheel: 9A alone matches nothing; with its neighbours, 8A does.
  await toolbar.getByRole('button', { name: /^Key/ }).click();
  const keys = page.getByRole('dialog', { name: 'Key filter' });
  await keys.getByRole('checkbox', { name: /^9A, E minor/ }).click();
  await expect(page.getByText('No tracks match these filters.')).toBeVisible();
  await keys.getByRole('checkbox', { name: /^Include keys that mix/ }).check();
  await expect.poll(() => columnValues(page, 'key')).toEqual(['8A']);
  // The wheel works from the keyboard: arrows move round it, Space toggles.
  const nineA = keys.getByRole('checkbox', { name: /^9A,/ });
  await nineA.focus();
  await page.keyboard.press(' ');
  await expect(nineA).toHaveAttribute('aria-checked', 'false');
  await page.keyboard.press('ArrowLeft');
  await expect(keys.getByRole('checkbox', { name: /^8A,/ })).toBeFocused();
  // Keys can be typed in any notation.
  await keys.getByRole('textbox', { name: 'Type a key' }).fill('A minor');
  await expect(keys).toContainText('A minor, 8A · 1m');
  await keys.getByRole('button', { name: 'Add' }).click();
  await expect(keys.getByRole('checkbox', { name: /^8A,/ })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(rows(page)).toHaveCount(total);

  // Another notation everywhere.
  await page.goto('/#/settings');
  await page.getByRole('radiogroup', { name: 'Key notation' }).getByRole('radio', { name: 'Open Key' }).click();
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(rows(page).first().locator('.tcell-key')).toHaveText('1m');
  await page.goto('/#/settings');
  await page.getByRole('radiogroup', { name: 'Key notation' }).getByRole('radio', { name: 'Camelot' }).click();
  await expect(page.getByRole('radiogroup', { name: 'Key notation' }).getByRole('radio', { name: 'Camelot' })).toHaveAttribute('aria-checked', 'true');
});

test('editing tempo, key and genre changes the library, never the file', async ({ page }) => {
  const file = join(MUSIC, 'other_track.flac');
  const before = readFileSync(file);
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'other_track').click();
  const panel = page.getByRole('complementary', { name: 'Track details' });
  await panel.getByRole('button', { name: 'Edit…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit BPM, key and genre' });
  await dialog.getByRole('button', { name: '×2' }).click();
  await expect(dialog.getByRole('textbox', { name: /^Tempo/ })).toHaveValue('256');
  await dialog.getByRole('textbox', { name: /^Key/ }).fill('Em');
  await expect(dialog).toContainText('E minor: 9A, 2m');
  // A text field with suggestions (a combobox).
  await dialog.getByRole('combobox', { name: /^Genre/ }).fill('Techno');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText('Updated 1 track')).toBeVisible();
  const r = row(page, 'other_track');
  await expect(r.locator('.tcell-bpm')).toHaveText('256');
  await expect(r.locator('.tcell-key')).toHaveText('9A');
  await expect(panel).toContainText('Techno');
  await expect(panel).toContainText('Edited');

  // The genre is now a filter. (Wait for the cleared search to apply first.)
  await page.getByRole('button', { name: 'Clear search' }).click();
  await expect.poll(() => rows(page).count()).toBeGreaterThan(1);
  await page.getByRole('toolbar', { name: 'Filters' }).getByRole('button', { name: /^Genre/ }).click();
  await page.getByRole('menuitemcheckbox', { name: /Techno/ }).click();
  await page.keyboard.press('Escape');
  await expect(rows(page)).toHaveCount(1);

  // Back to what was detected.
  await row(page, 'other_track').click();
  await panel.getByRole('button', { name: 'Edit…' }).click();
  // Resetting drops every edit at once: it asks first, in the dialog.
  await dialog.getByRole('button', { name: 'Reset to detected…' }).click();
  await expect(dialog).toContainText('Remove your edits and Discogs genres?');
  await expect(dialog.getByRole('button', { name: 'Keep them' })).toBeFocused();
  await dialog.getByRole('button', { name: 'Reset to detected', exact: true }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(row(page, 'other_track').locator('.tcell-bpm')).toHaveText('128');
  await expect(row(page, 'other_track').locator('.tcell-key')).toHaveText('8A');
  expect(readFileSync(file).equals(before)).toBe(true);
});

test('DJ export writes tagged files and a rekordbox playlist, and never overwrites', async ({ page }) => {
  const out = join(DATA, 'export');
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'other_track').click();
  await page.getByRole('complementary', { name: 'Track details' }).getByRole('button', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Export for DJ software' });
  await expect(dialog).toContainText('1 file will be written');
  await expect(dialog.getByRole('radio', { name: 'AIFF' })).toHaveAttribute('aria-checked', 'true');
  await dialog.getByRole('textbox', { name: 'Export folder (full path)' }).fill(out);
  await dialog.getByRole('button', { name: 'Export 1 file' }).click();
  const done = page.getByRole('dialog', { name: 'Export finished' });
  await expect(done).toContainText('Exported 1 file');
  await expect(done).toContainText('rekordbox');
  await done.getByRole('button', { name: 'Done' }).click();
  expect(existsSync(join(out, 'other_track.aiff'))).toBe(true);
  const xml = readFileSync(join(out, 'rekordbox.xml'), 'utf8');
  expect(xml).toContain('Tonality="8A"');
  expect(xml).toContain('AverageBpm="128');

  // A second export never replaces the first.
  await page.getByRole('complementary', { name: 'Track details' }).getByRole('button', { name: 'Export…' }).click();
  await dialog.getByRole('textbox', { name: 'Export folder (full path)' }).fill(out);
  await dialog.getByRole('button', { name: 'Export 1 file' }).click();
  await expect(done).toContainText('Exported 1 file');
  await done.getByRole('button', { name: 'Done' }).click();
  expect(existsSync(join(out, 'other_track (2).aiff'))).toBe(true);
  rmSync(out, { recursive: true, force: true });
});

test('the export plan follows the format: MP3 never warns about a resolution choice it does not show', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clean_master_24bit');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'clean_master_24bit').click();
  await page.getByRole('complementary', { name: 'Track details' }).getByRole('button', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Export for DJ software' });
  // AIFF at 16-bit / 44.1 kHz: a 24-bit source is downsampled, and the plan says so.
  await dialog.getByRole('radio', { name: 'AIFF' }).click();
  await dialog.getByRole('radio', { name: '16-bit / 44.1 kHz' }).click();
  await expect(dialog).toContainText('1 file with notes');
  await dialog.getByRole('button', { name: 'Show details' }).click();
  await expect(dialog).toContainText('Reduced to 16-bit/44.1 kHz');
  // MP3 has no quality choice, so the hidden one must not shape its plan.
  await dialog.getByRole('radio', { name: 'MP3 320' }).click();
  await expect(dialog.getByRole('radio', { name: '16-bit / 44.1 kHz' })).toHaveCount(0);
  await expect(dialog).toContainText('1 file will be written');
  await expect(dialog).not.toContainText('with notes');
  // Back to AIFF, the note returns: the choice was kept, not reset.
  await dialog.getByRole('radio', { name: 'AIFF' }).click();
  await expect(dialog.getByRole('radio', { name: '16-bit / 44.1 kHz' })).toHaveAttribute('aria-checked', 'true');
  await expect(dialog).toContainText('1 file with notes');
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();
});

test('tracks play inside the app; formats the window cannot read play from a decoded copy', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('mp3_320');
  // mp3_320 and transcode_mp3_320.
  await expect(rows(page)).toHaveCount(2);
  await row(page, 'mp3_320').click();
  await page.keyboard.press('p');
  const player = page.getByRole('region', { name: 'Player' });
  await expect(player).toContainText('mp3_320');
  await expect(player.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
  await expect(player.getByRole('slider', { name: 'Position' })).not.toHaveAttribute('aria-valuenow', '0', { timeout: 10_000 });
  await expect(player).not.toContainText('decoded copy');
  await page.keyboard.press('p');
  await expect(player.getByRole('button', { name: 'Play', exact: true })).toBeVisible();

  // Chromium cannot play AIFF: SONORA decodes it once and plays that.
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('clean_master');
  const aiff = rows(page).filter({ has: page.locator('.fmt-name', { hasText: 'AIFF' }) });
  await aiff.getByRole('button', { name: /^Play clean_master/ }).click();
  await expect(player.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
  await expect(player).toContainText('decoded copy');
  // Seeking with the keyboard.
  await player.getByRole('slider', { name: 'Position' }).focus();
  await page.keyboard.press('End');
  await expect(player.getByRole('slider', { name: 'Position' })).toHaveAttribute('aria-valuenow', /^(19|20)$/);
  await player.getByRole('button', { name: 'Close the player' }).click();
  await expect(player).toBeHidden();
});

test('genre lookups ask for a Discogs token first and send nothing without one', async ({ page }) => {
  await page.goto('/#/library');
  await page.getByRole('searchbox', { name: 'Search tracks' }).fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'other_track').click();
  await page.getByRole('complementary', { name: 'Track details' }).getByRole('button', { name: 'Discogs' }).click();
  const dialog = page.getByRole('dialog', { name: 'Connect Discogs first' });
  await expect(dialog).toContainText('stays on this computer');
  await dialog.getByRole('button', { name: 'Open Settings' }).click();
  await expect(page.getByRole('textbox', { name: 'Discogs personal access token' })).toBeVisible();
  await expect(page.getByText('not affiliated with, sponsored or endorsed by Discogs')).toBeVisible();
});

test('updates: found on request, explained, installed with a restart; the browser build says it cannot update', async ({ page }) => {
  await page.goto('/#/settings');
  await expect(page.getByText('This window cannot update SONORA')).toBeVisible();

  // The browser bridge simulates a release when asked to (development and tests only).
  await page.goto('/?simulate-update=9.9.9#/settings');
  const updates = page.getByRole('region', { name: 'Updates' });
  await expect(updates.getByText('Not checked yet.')).toBeVisible();
  await updates.getByRole('button', { name: 'Check now' }).click();
  const dialog = page.getByRole('dialog', { name: 'SONORA 9.9.9 is available' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('listitem').filter({ hasText: 'Playlists you can sort by dragging' })).toBeVisible();
  await expect(dialog).toContainText('Your library, settings and edits are kept');
  await dialog.getByRole('button', { name: 'Later' }).click();
  await expect(dialog).toBeHidden();
  await expect(updates.getByText('Version 9.9.9 is available.')).toBeVisible();

  // The status bar offers it from anywhere.
  await page.goto('/?simulate-update=9.9.9#/library');
  await page.getByRole('button', { name: 'Update to 9.9.9' }).click();
  const reloaded = page.waitForEvent('load');
  await page.getByRole('button', { name: 'Install and restart' }).click();
  await expect(page.getByRole('dialog', { name: 'Updating SONORA' })).toContainText(/Downloading|Checking the signature|Restarting/);
  await reloaded;

  // Automatic checks can be turned off, and stay off.
  await page.goto('/?simulate-update=9.9.9#/settings');
  const auto = page.getByRole('switch', { name: /Check for updates automatically/ });
  await expect(auto).toBeChecked();
  // Saved by the core first, then shown.
  await auto.click();
  await expect(auto).not.toBeChecked();
  await page.reload();
  await expect(auto).not.toBeChecked();
  await auto.click();
  await expect(auto).toBeChecked();
});

test('updates: a failed automatic check keeps the offer, and FFmpeg\'s Check again answers without an extra update check', async ({ page }) => {
  // The simulated updater asks the core for app.info on each check; nothing
  // else calls it after the first load except "Check again".
  let infoCalls = 0;
  let offline = false;
  await page.route('**/api/call', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body.method === 'app.info') {
      infoCalls += 1;
      if (offline) return route.fulfill({ status: 503, body: 'offline' });
    }
    return route.continue();
  });
  await page.clock.install();
  await page.goto('/?simulate-update=9.9.9#/settings');
  const updates = page.getByRole('region', { name: 'Updates' });
  await expect(updates.getByText('Not checked yet.')).toBeVisible();
  // The first automatic check, ten seconds after launch, finds the release.
  await page.clock.fastForward(10_500);
  await page.clock.fastForward(1_000);
  await expect(updates.getByText('Version 9.9.9 is available.')).toBeVisible();

  // B83: six hours on, the check fails (offline); the release is still on offer.
  offline = true;
  await page.clock.fastForward(6 * 60 * 60 * 1000);
  await page.clock.fastForward(1_000);
  await expect.poll(() => infoCalls).toBeGreaterThanOrEqual(3);
  await expect(updates.getByText('Version 9.9.9 is available.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Update to 9.9.9' })).toBeVisible();
  offline = false;

  // B85: "Check again" says what it found. B84: and does not restart the
  // update timers (a new check ten seconds later).
  await page.getByRole('button', { name: 'Check again' }).click();
  await expect(page.getByRole('status').filter({ hasText: /FFmpeg (found|is still not found)/ })).toBeVisible();
  const after = infoCalls;
  await page.clock.fastForward(10_500);
  await page.clock.fastForward(1_000);
  await page.waitForTimeout(500);
  expect(infoCalls).toBe(after);
});

test('the browser build reads the jobs again when its event stream reconnects (B96)', async ({ page }) => {
  // Keep a handle on the page's event stream, to play a lost event and a reconnection.
  await page.addInitScript(() => {
    const Original = window.EventSource;
    const streams: EventSource[] = [];
    (window as unknown as { streams: EventSource[] }).streams = streams;
    window.EventSource = class extends Original {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        streams.push(this);
      }
    };
  });
  await page.goto('/#/library');
  await page.waitForFunction(() => (window as unknown as { streams: EventSource[] }).streams.some((s) => s.readyState === 1));
  // An export seen starting, whose "finished" never arrives.
  await page.evaluate(() => {
    for (const s of (window as unknown as { streams: EventSource[] }).streams) {
      s.dispatchEvent(new MessageEvent('djexport.progress', { data: JSON.stringify({ done: 0, total: 3, fraction: 0.1, current: 'a.flac' }) }));
    }
  });
  const exporting = page.getByRole('status').filter({ hasText: 'Exporting' });
  await expect(exporting).toBeVisible();
  // The stream comes back: the core says no export runs.
  await page.evaluate(() => {
    for (const s of (window as unknown as { streams: EventSource[] }).streams) s.dispatchEvent(new Event('open'));
  });
  await expect(exporting).toHaveCount(0);
});


test('playlists: made in the sidebar, filled by dragging or from the details panel, put in order', async ({ page }) => {
  await page.goto('/#/library');
  await expect(page.getByText('Sort tracks into sets, then send them to a USB drive.')).toBeVisible();

  // A new playlist opens with its name ready to type.
  await page.getByRole('button', { name: 'New playlist, folder or smart playlist' }).click();
  await page.getByRole('menuitem', { name: 'New playlist', exact: true }).click();
  const naming = page.getByRole('textbox', { name: 'New name for New playlist' });
  await expect(naming).toBeFocused();
  await naming.fill('Warm-up');
  await naming.press('Enter');
  const tree = page.getByRole('tree', { name: 'Playlists' });
  const warmup = tree.getByRole('treeitem', { name: /Warm-up/ });
  await expect(warmup).toHaveAttribute('aria-current', 'page');
  await expect(warmup).toBeFocused();
  await expect(page.locator('.view-title')).toHaveText('Warm-up');
  await expect(page.getByText('In the library, drag tracks onto “Warm-up” in the sidebar')).toBeVisible();
  await page.getByRole('button', { name: 'Go to the library' }).click();

  // Tracks dragged onto it in the sidebar, each once.
  const search = page.getByRole('searchbox', { name: 'Search tracks' });
  await search.fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await drag(page, row(page, 'other_track').locator('.tcell-bpm'), warmup);
  await expect(page.getByText('Added 1 track to “Warm-up”')).toBeVisible();
  await expect(warmup.locator('.pl-count')).toHaveText('1');
  await drag(page, row(page, 'other_track').locator('.tcell-bpm'), warmup);
  await expect(page.getByText('Already in “Warm-up”')).toBeVisible();
  await search.fill('radio_edit');
  await expect(rows(page)).toHaveCount(1);
  await drag(page, row(page, 'radio_edit').locator('.tcell-bpm'), warmup);
  await expect(warmup.locator('.pl-count')).toHaveText('2');

  // Or from the details panel, which then lists the playlists holding the track.
  await search.fill('dark_master');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'dark_master').click();
  const details = page.getByRole('complementary', { name: 'Track details' });
  await details.getByRole('button', { name: 'Add to playlist' }).click();
  await page.getByRole('menuitem', { name: /^Warm-up/ }).click();
  await expect(warmup.locator('.pl-count')).toHaveText('3');
  await expect(details.getByRole('link', { name: 'Warm-up' })).toBeVisible();

  // A playlist made of a selection.
  await search.fill('master');
  await row(page, 'clipped_master').click();
  await row(page, 'dark_master').click({ modifiers: ['Control'] });
  const selection = page.getByRole('complementary', { name: 'Selection' });
  await expect(selection).toContainText('2 tracks selected');
  await selection.getByRole('button', { name: 'Add to playlist' }).click();
  await page.getByRole('menuitem', { name: 'New playlist of 2 tracks' }).click();
  const naming2 = page.getByRole('textbox', { name: 'New name for New playlist' });
  await expect(naming2).toBeFocused();
  await naming2.fill('Peak time');
  await naming2.press('Enter');
  const peak = tree.getByRole('treeitem', { name: /Peak time/ });
  await expect(peak.locator('.pl-count')).toHaveText('2');

  // The playlist: the set at a glance, then its tracks in order.
  await warmup.click();
  const set = page.getByRole('img', { name: /^The set: 3 tracks/ });
  await expect(set).toBeVisible();
  await expect.poll(() => playlistTitles(page)).toEqual(['other_track', 'radio_edit', 'dark_master']);
  await expect(row(page, 'other_track').locator('.tcell-mix')).toHaveText('Opener');
  // A block on the set picks its track, outlined there too.
  const box = await set.boundingBox();
  if (!box) throw new Error('no timeline');
  await page.mouse.click(box.x + box.width * 0.95, box.y + 20);
  await expect(row(page, 'dark_master')).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.tl-selected')).toHaveCount(1);

  // Alt+↑ moves a track up a place; dragging moves it anywhere. The order is kept.
  await row(page, 'dark_master').click();
  await page.keyboard.press('Alt+ArrowUp');
  await expect.poll(() => playlistTitles(page)).toEqual(['other_track', 'dark_master', 'radio_edit']);
  await drag(page, row(page, 'radio_edit').locator('.tcell-bpm'), row(page, 'other_track'), { y: 4 });
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'other_track', 'dark_master']);
  await page.reload();
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'other_track', 'dark_master']);

  // Delete takes a track out (the library keeps it); Undo puts it back in its place.
  await row(page, 'other_track').click();
  await page.keyboard.press('Delete');
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'dark_master']);
  await expect(page.getByText('Removed 1 track from the playlist')).toBeVisible();
  await page.getByRole('button', { name: 'Undo' }).click();
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'other_track', 'dark_master']);

  // Arranging shows the set before and after, and changes nothing unless asked.
  await page.getByRole('button', { name: 'Arrange for mixing…' }).click();
  const arrange = page.getByRole('dialog', { name: 'Arrange for mixing' });
  await expect(arrange.getByRole('img', { name: 'The set in the arranged order' })).toBeVisible();
  await expect(arrange).toContainText('Key clashes');
  await arrange.getByRole('button', { name: 'Cancel' }).click();
  await expect(arrange).toBeHidden();
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'other_track', 'dark_master']);

  // Playlists move among their neighbours from the keyboard too, keeping the focus.
  await warmup.focus();
  await page.keyboard.press('Alt+ArrowDown');
  await expect(tree.getByRole('treeitem').first()).toContainText('Peak time');
  await expect(warmup).toBeFocused();
  await page.keyboard.press('Alt+ArrowUp');
  await expect(tree.getByRole('treeitem').first()).toContainText('Warm-up');
});

test('smart playlists keep up with their rules; folders hold playlists', async ({ page }) => {
  // The library's filters, saved as a smart playlist.
  await page.goto('/#/library');
  const toolbar = page.getByRole('toolbar', { name: 'Filters' });
  await toolbar.getByRole('button', { name: /^BPM/ }).click();
  const tempo = page.getByRole('dialog', { name: 'Tempo filter' });
  await tempo.getByRole('textbox', { name: 'Lowest BPM' }).fill('126');
  await tempo.getByRole('textbox', { name: 'Highest BPM' }).fill('130');
  await page.keyboard.press('Escape');
  await expect.poll(() => columnValues(page, 'bpm')).toEqual(['128']);
  const matching = await rows(page).count();
  await page.getByRole('button', { name: 'Save as smart playlist' }).click();
  const rules = page.getByRole('dialog', { name: 'New smart playlist' });
  await expect(rules.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('126–130 BPM');
  await expect(rules).toContainText(matching === 1 ? '1 track matches now' : `${matching} tracks match now`);
  await rules.getByRole('button', { name: 'Make smart playlist' }).click();
  const tree = page.getByRole('tree', { name: 'Playlists' });
  const smart = tree.getByRole('treeitem', { name: /126–130 BPM/ });
  await expect(smart).toHaveAttribute('aria-current', 'page');
  await expect(rows(page)).toHaveCount(matching);

  // Wider rules, more tracks.
  await page.getByRole('button', { name: 'Edit rules…' }).click();
  const edit = page.getByRole('dialog', { name: 'Edit smart playlist' });
  await edit.getByRole('textbox', { name: 'Lowest BPM' }).fill('120');
  await edit.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(edit).toBeHidden();
  await expect(page.locator('.view-sub')).toContainText('120–130 BPM');
  await expect.poll(() => rows(page).count()).toBeGreaterThan(matching);

  // Converted (after a question: the rules go), it is an ordinary playlist;
  // deleting it asks first and keeps the tracks.
  await page.getByRole('button', { name: 'More playlist actions' }).click();
  await page.getByRole('menuitem', { name: 'Convert to a playlist' }).click();
  const convert = page.getByRole('dialog', { name: 'Convert “126–130 BPM” to a playlist?' });
  await expect(convert).toContainText('its rules are not kept');
  await convert.getByRole('button', { name: 'Convert to a playlist' }).click();
  await expect(page.getByText(/^Now a playlist of \d+ tracks$/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Arrange for mixing…' })).toBeVisible();
  await page.getByRole('button', { name: 'More playlist actions' }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const confirm = page.getByRole('dialog', { name: 'Delete the playlist “126–130 BPM”?' });
  await expect(confirm).toContainText('The tracks stay in your library');
  await confirm.getByRole('button', { name: 'Delete playlist' }).click();
  await expect(smart).toBeHidden();
  await expect(page).toHaveURL(/#\/library/);

  // A folder, and playlists dragged into it.
  await page.getByRole('button', { name: 'New playlist, folder or smart playlist' }).click();
  await page.getByRole('menuitem', { name: 'New folder' }).click();
  const naming = page.getByRole('textbox', { name: 'New name for New folder' });
  await expect(naming).toBeFocused();
  await naming.fill('Friday');
  await naming.press('Enter');
  const friday = tree.getByRole('treeitem', { name: /Friday/ });
  await expect(friday).toHaveAttribute('aria-level', '1');
  const warmup = tree.getByRole('treeitem', { name: /Warm-up/ });
  const peak = tree.getByRole('treeitem', { name: /Peak time/ });
  await drag(page, warmup, friday);
  await expect(warmup).toHaveAttribute('aria-level', '2');
  await expect(friday).toHaveAttribute('aria-expanded', 'true');
  await drag(page, peak, friday);
  await expect(peak).toHaveAttribute('aria-level', '2');
  // From the keyboard: Alt+← takes a playlist out of its folder, Alt+→ puts
  // it into the folder above it.
  await peak.focus();
  await page.keyboard.press('Alt+ArrowLeft');
  await expect(peak).toHaveAttribute('aria-level', '1');
  await expect(peak).toBeFocused();
  await page.keyboard.press('Alt+ArrowRight');
  await expect(peak).toHaveAttribute('aria-level', '2');
  await expect(peak).toBeFocused();

  await friday.click();
  await expect(page.locator('.view-title')).toHaveText('Friday');
  await expect(page.locator('.view-sub')).toHaveText('2 items, 5 tracks');
  await expect(page.locator('.folder-list')).toContainText('Warm-up');

  // ← closes a folder and → opens it.
  await friday.focus();
  await page.keyboard.press('ArrowLeft');
  await expect(friday).toHaveAttribute('aria-expanded', 'false');
  await expect(warmup).toBeHidden();
  await page.keyboard.press('ArrowRight');
  await expect(warmup).toBeVisible();

  // A row's menu acts on that row and does not open it.
  const here = page.url();
  await tree.getByRole('button', { name: 'Actions for Peak time' }).click();
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  const renaming = page.getByRole('textbox', { name: 'New name for Peak time' });
  await expect(renaming).toBeFocused();
  expect(page.url()).toBe(here);
  await renaming.press('Escape');
  await expect(peak).toBeFocused();
  await expect(peak).toHaveAccessibleName('Peak time, 2 tracks');
});

test('Send to USB writes numbered folders of DJ-ready files and playlists, and changes only the files it wrote', async ({ page }) => {
  const usb = join(DATA, 'usb');
  rmSync(usb, { recursive: true, force: true });
  mkdirSync(usb, { recursive: true });
  // The owner's own file on the drive.
  writeFileSync(join(usb, 'notes.txt'), 'mine');

  await page.goto('/#/library');
  const tree = page.getByRole('tree', { name: 'Playlists' });
  await tree.getByRole('treeitem', { name: /Friday/ }).click();
  await page.getByRole('button', { name: 'Send to USB…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Send playlists to a USB drive' });
  const picker = dialog.getByRole('group', { name: 'Playlists to send' });
  await expect(picker.getByRole('checkbox', { name: /Friday/ })).toBeChecked();
  // What is inside a chosen folder goes with it.
  await expect(picker.getByRole('checkbox', { name: /Warm-up/ })).toBeChecked();
  await expect(picker.getByRole('checkbox', { name: /Warm-up/ })).toBeDisabled();
  await dialog.getByRole('textbox', { name: 'Drive or folder (full path)' }).fill(usb);
  const plan = dialog.getByRole('region', { name: 'What happens' });
  await expect(plan).toContainText('5 files to write');
  await expect(dialog.getByRole('region', { name: 'Files on the drive' })).toContainText('SONORA/01 Friday/01 Warm-up');
  // Serato finds crates only at the top of a drive.
  await expect(dialog).toContainText('only when sending to the top of a drive');
  await dialog.getByRole('button', { name: 'Send to usb' }).click();
  const done = page.getByRole('dialog', { name: 'Sent to the drive' });
  await expect(done).toContainText('Sent 2 playlists to the drive', { timeout: 60_000 });
  await expect(done).toContainText('SONORA/rekordbox.xml');
  // B50: when the folder cannot be shown, the dialog says why.
  await page.route('**/api/call', async (route) => {
    const b = route.request().postDataJSON() as { method?: string };
    if (b.method === 'system.reveal') return route.fulfill({ json: { ok: false, error: { code: 'not_found', message: 'The drive is not connected.' } } });
    return route.continue();
  });
  await done.getByRole('button', { name: 'Show in folder' }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'The drive is not connected.' })).toBeVisible();
  await page.unroute('**/api/call');
  await done.getByRole('button', { name: 'Done' }).click();

  const warm = join(usb, 'SONORA', '01 Friday', '01 Warm-up');
  expect(readdirSync(warm).sort()).toEqual(['001 - radio_edit.aiff', '002 - other_track.aiff', '003 - dark_master.aiff', 'Warm-up.m3u8']);
  const m3u = readFileSync(join(warm, 'Warm-up.m3u8'), 'utf8');
  expect(m3u.startsWith('#EXTM3U\n')).toBe(true);
  expect(m3u.indexOf('001 - radio_edit.aiff')).toBeLessThan(m3u.indexOf('002 - other_track.aiff'));
  expect(readdirSync(join(usb, 'SONORA', '01 Friday', '02 Peak time')).filter((f) => f.endsWith('.aiff'))).toHaveLength(2);
  const xml = readFileSync(join(usb, 'SONORA', 'rekordbox.xml'), 'utf8');
  // rekordbox lists the playlists by their names on the drive, in the same order.
  expect(xml).toContain('Name="01 Friday"');
  expect(xml).toContain('Name="01 Warm-up"');
  expect(existsSync(join(usb, 'SONORA', '.sonora-sync.json'))).toBe(true);

  // A track taken out of the playlist: its file goes only when asked, and
  // files that are not SONORA's stay, even in its folders.
  writeFileSync(join(warm, 'my edit.aiff'), 'mine too');
  await tree.getByRole('treeitem', { name: /Warm-up/ }).click();
  await row(page, 'other_track').click();
  await page.keyboard.press('Delete');
  await expect.poll(() => playlistTitles(page)).toEqual(['radio_edit', 'dark_master']);
  await page.getByRole('button', { name: 'Send to USB…' }).click();
  await dialog.getByRole('textbox', { name: 'Drive or folder (full path)' }).fill(usb);
  await expect(plan).toContainText('1 to move, 1 already there');
  await dialog.getByRole('checkbox', { name: /^Remove the 1 file/ }).check();
  await dialog.getByRole('button', { name: 'Send to usb' }).click();
  await expect(done).toContainText('Sent 1 playlist to the drive', { timeout: 60_000 });
  await done.getByRole('button', { name: 'Done' }).click();
  expect(readdirSync(warm).sort()).toEqual(['001 - radio_edit.aiff', '002 - dark_master.aiff', 'Warm-up.m3u8', 'my edit.aiff']);
  expect(readFileSync(join(usb, 'notes.txt'), 'utf8')).toBe('mine');
  rmSync(usb, { recursive: true, force: true });
});

test('Soulseek: search through slskd, download, and the files join the library with their quality checked', async ({ page }) => {
  // A stand-in for slskd: nothing here reaches Soulseek.
  const downloads = join(DATA, 'slskd-downloads');
  rmSync(downloads, { recursive: true, force: true });
  mkdirSync(downloads, { recursive: true });
  const album = '@@shared\\Music\\Night Drive - Lights';
  const slskd = await startFakeSlskd([
    { username: 'crate-digger', filename: `${album}\\01 Lights.flac`, size: statSync(join(MUSIC, 'clean_master.flac')).size, bitDepth: 16, sampleRate: 44100, length: 20 },
    { username: 'crate-digger', filename: `${album}\\02 Afterglow.flac`, size: statSync(join(MUSIC, 'transcode_mp3_128.flac')).size, bitDepth: 16, sampleRate: 44100, length: 20 },
    { username: 'mp3-only', filename: 'mp3s\\Night Drive\\01 Lights.mp3', size: 480_000, bitRate: 192, length: 20 },
  ]);
  try {
    // Nothing works before the notice is accepted.
    await page.goto('/#/soulseek');
    await expect(page.getByRole('heading', { name: 'Find lossless music on Soulseek' })).toBeVisible();
    await expect(page.getByRole('note', { name: 'Before you start' })).toContainText('you are responsible for making sure each download');
    await page.getByRole('button', { name: 'Accept and continue' }).click();

    // Setup by hand: slskd's address, a key SONORA makes, then the download folder.
    await expect(page.getByRole('tab', { name: 'Setup' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('heading', { name: 'Set up Soulseek for me' })).toBeVisible();
    await page.getByRole('button', { name: 'I’ll set up slskd myself' }).click();
    const address = page.getByRole('textbox', { name: 'slskd’s address' });
    await address.fill(slskd.url);
    await address.press('Enter');
    await page.getByRole('button', { name: 'Make a key' }).click();
    const yaml = page.getByLabel('Lines for slskd.yml');
    await expect(yaml).toContainText('role: readwrite');
    await expect(yaml).not.toContainText('cidr');
    const key = /key: (\S+)/.exec(await yaml.innerText())?.[1] ?? '';
    expect(key).toMatch(/^[A-Za-z0-9]{32}$/);
    slskd.key = key;
    await page.getByRole('button', { name: 'I restarted slskd: test' }).click();
    await expect(page.locator('.view-sub')).toContainText('Signed in to Soulseek as e2e-dj through slskd 0.26.1');
    await page.getByRole('textbox', { name: 'Download folder path' }).fill(downloads);
    await page.getByRole('button', { name: 'Use folder' }).click();
    await expect(page.getByText(downloads, { exact: true })).toBeVisible();

    // Search: lossless files only, grouped by folder, best-connected first.
    // B19: the first look at the results fails (slskd busy); the search goes on.
    let failed = 0;
    await page.route('**/api/call', async (route) => {
      const b = route.request().postDataJSON() as { method?: string };
      if (b.method === 'slskd.results' && failed === 0) {
        failed += 1;
        return route.fulfill({ status: 503, body: 'busy' });
      }
      return route.continue();
    });
    await page.getByRole('tab', { name: 'Search' }).click();
    await page.getByRole('searchbox', { name: 'Search Soulseek' }).fill('night drive lights');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    const folder = page.getByRole('region', { name: 'Night Drive - Lights from crate-digger' });
    await expect(folder.getByRole('cell', { name: '01 Lights.flac', exact: true })).toBeVisible();
    expect(failed).toBe(1);
    await page.unroute('**/api/call');
    await expect(page.locator('.sk-summary')).toContainText('1 lossy file hidden', { timeout: 15_000 });
    expect(slskd.searches).toEqual(['night drive lights']);
    await expect(page.getByRole('region', { name: /from mp3-only/ })).toHaveCount(0);
    await page.getByRole('radio', { name: 'All formats' }).click();
    await expect(page.getByRole('region', { name: 'Night Drive from mp3-only' })).toBeVisible();
    await page.getByRole('radio', { name: 'Lossless' }).click();
    await expect(page.getByRole('region', { name: /from mp3-only/ })).toHaveCount(0);

    // Download the folder; slskd queues both files.
    await folder.getByRole('button', { name: 'Download 2 files' }).click();
    await expect(page.getByText('Asked crate-digger for 2 files')).toBeVisible();
    await expect(folder.getByText('Queued')).toHaveCount(2);
    expect(slskd.transfers.map((t) => t.filename).sort()).toEqual([`${album}\\01 Lights.flac`, `${album}\\02 Afterglow.flac`]);
    await page.getByRole('tab', { name: /^Downloads/ }).click();
    await expect(page.getByText('Number 2 in their queue')).toBeVisible();

    // slskd finishes them in its folder: SONORA adds both and checks them.
    // The second is an MP3 saved as FLAC, and the analysis says so.
    const dir = join(downloads, 'Night Drive - Lights');
    mkdirSync(dir, { recursive: true });
    copyFileSync(join(MUSIC, 'clean_master.flac'), join(dir, '01 Lights.flac'));
    copyFileSync(join(MUSIC, 'transcode_mp3_128.flac'), join(dir, '02 Afterglow.flac'));
    const settled = new Date(Date.now() - 60_000);
    for (const f of readdirSync(dir)) utimesSync(join(dir, f), settled, settled);
    slskd.finish(`${album}\\01 Lights.flac`);
    slskd.finish(`${album}\\02 Afterglow.flac`);
    await expect(page.getByText('Added 2 downloads from Soulseek')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('Files from Soulseek come from other people')).toBeVisible();
    await waitIdle(page);
    const lights = page.getByRole('row', { name: /01 Lights\.flac/ });
    const afterglow = page.getByRole('row', { name: /02 Afterglow\.flac/ });
    await expect(lights.locator('.sk-verdict')).toBeVisible({ timeout: 30_000 });
    await expect(afterglow.locator('.sk-verdict')).toContainText('Lossy source?');
    await expect(lights.locator('.sk-verdict')).not.toContainText('Lossy source?');

    // Later results show the files as already in the library.
    await page.getByRole('tab', { name: 'Search' }).click();
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Night Drive - Lights from crate-digger' }).getByText('Downloaded')).toHaveCount(2, { timeout: 15_000 });
    // B58: not "All asked for".
    await expect(page.getByRole('region', { name: 'Night Drive - Lights from crate-digger' }).getByRole('button', { name: 'All in your library' })).toBeVisible();

    // The analysis page says where the track came from.
    await page.getByRole('tab', { name: /^Downloads/ }).click();
    await lights.getByRole('link', { name: 'Open the analysis of 01 Lights.flac' }).click();
    await expect(page.getByText('Came from')).toBeVisible();
    await expect(page.getByText(/^Soulseek, /)).toBeVisible();

    // slskd's files stay exactly where it put them.
    expect(readdirSync(dir).sort()).toEqual(['01 Lights.flac', '02 Afterglow.flac']);
  } finally {
    await slskd.close();
  }
});

test('Soulseek: one button sets slskd up, a failed step can be tried again, and Repair puts the key back', async ({ page }) => {
  // SONORA's own slskd answers on MANAGED_SLSKD_PORT; here a stand-in that
  // reads the slskd.yml SONORA writes, as slskd does. Nothing reaches GitHub,
  // launchd or Soulseek (SONORA_SLSKD_SYSTEM=test in e2e/serve.mjs).
  const config = join(LIBRARY, 'slskd', 'data', 'slskd.yml');
  const downloads = join(DATA, 'managed-downloads');
  rmSync(downloads, { recursive: true, force: true });
  // The notice was accepted on screen in the test before; accepted here
  // too, so this test also runs on its own.
  await page.goto('/#/soulseek');
  await api(page, 'sources.accept');
  await page.reload();
  await page.getByRole('tab', { name: 'Setup' }).click();

  // B04: an earlier slskd that shared a folder, and another slskd running.
  // The share is offered once; turned off, it stays off after "Stop it".
  let other = true;
  const shared = join(DATA, 'review-share');
  await page.route('**/api/call', async (route) => {
    const b = route.request().postDataJSON() as { method?: string };
    if (b.method === 'slskd.stopOther') {
      other = false;
      return route.fulfill({ json: { ok: true, result: null } });
    }
    if (b.method !== 'slskd.setupPlan') return route.continue();
    const res = await route.fetch();
    const j = (await res.json()) as { ok: boolean; result?: { share: string | null; others: { pid: number; path: string }[] } };
    if (j.ok && j.result) {
      j.result.share = shared;
      j.result.others = other ? [{ pid: 424242, path: '/usr/local/bin/slskd' }] : [];
    }
    return route.fulfill({ response: res, json: j });
  });
  await page.getByRole('button', { name: 'Set up Soulseek…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Set up Soulseek' });
  const sharing = dialog.getByRole('switch', { name: 'Share a folder with other Soulseek users' });
  await expect(sharing).toBeChecked();
  await sharing.click();
  await expect(sharing).not.toBeChecked();
  await dialog.getByRole('button', { name: 'Stop it' }).click();
  await expect(dialog.getByRole('button', { name: 'Stop it' })).toHaveCount(0);
  await expect(sharing).not.toBeChecked();
  await page.unroute('**/api/call');

  // One permission screen that says exactly what will happen.
  await expect(dialog.getByText('Check it against the checksum built into SONORA, and stop if it differs.')).toBeVisible();
  await expect(dialog.getByText(/It shares nothing\./)).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Allow and set up' })).toBeDisabled();
  await dialog.getByRole('textbox', { name: 'Soulseek user name' }).fill('e2e-dj');
  await dialog.getByLabel('Password').fill('right');
  await dialog.getByRole('textbox', { name: 'Folder for finished downloads' }).fill(downloads);
  await dialog.getByRole('button', { name: 'Allow and set up' }).click();

  // slskd does not answer yet: the step that failed says so, and can be tried again.
  const steps = page.getByRole('region', { name: 'Setting up slskd' });
  await expect(steps.getByText('Starting slskd did not work.')).toBeVisible({ timeout: 30_000 });
  await expect(steps.locator('.sk-step.is-done')).toHaveCount(4);
  expect(existsSync(config)).toBe(true);
  const slskd = await startFakeSlskd([], { port: MANAGED_SLSKD_PORT, config });
  try {
    await steps.getByRole('button', { name: 'Try again' }).click();
    await expect(steps.getByText('Signed in to Soulseek as e2e-dj.', { exact: false })).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.view-sub')).toContainText('Signed in to Soulseek as e2e-dj', { timeout: 15_000 });

    // SONORA keeps slskd running, and watches the folder it saves to.
    const panel = page.getByRole('region', { name: 'slskd', exact: true });
    await expect(panel.getByRole('switch', { name: 'Run slskd in the background' })).toBeChecked();
    // The path of slskd.yml breaks inside its column instead of running over the next one.
    const intro = panel.locator('.settings-desc');
    await expect(intro).toContainText('slskd.yml');
    expect(await intro.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
    // The password of slskd's web page, from slskd.yml, goes to the clipboard.
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await panel.getByRole('button', { name: 'Copy web password' }).click();
    await expect(page.getByText('Copied slskd’s web password')).toBeVisible();
    const webPassword = /^ {4}password: '([^']+)'$/m.exec(readFileSync(config, 'utf8'))?.[1];
    expect(webPassword).toBeTruthy();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(webPassword);
    await expect(page.getByText(downloads, { exact: true })).toBeVisible();
    expect(readFileSync(config, 'utf8')).toContain(`downloads: '${downloads}'`);

    // slskd.yml lost SONORA's key: Repair puts it back and slskd answers again.
    const yml = readFileSync(config, 'utf8');
    writeFileSync(config, yml.split('\n').filter((l) => !l.trimStart().startsWith('key:')).join('\n'));
    await panel.getByRole('button', { name: 'Restart' }).click();
    await panel.getByRole('button', { name: 'Repair' }).click();
    await expect(page.getByText('Repaired: slskd has SONORA’s key again')).toBeVisible();
    expect(readFileSync(config, 'utf8')).toMatch(/^ {8}key: '[A-Za-z0-9]{16,}'$/m);
    await expect(page.locator('.view-sub')).toContainText('Signed in to Soulseek as e2e-dj', { timeout: 15_000 });
  } finally {
    await slskd.close();
  }
});

test('Soulseek previews play while the file arrives, are never downloads, queue up, and leave nothing behind', async ({ page }) => {
  // A stand-in slskd that "receives" files a piece at a time into its
  // folder for partial downloads, as slskd does. Nothing reaches Soulseek.
  const root = join(DATA, 'preview-slskd');
  rmSync(root, { recursive: true, force: true });
  const downloads = join(root, 'downloads');
  const incomplete = join(root, 'incomplete');
  mkdirSync(downloads, { recursive: true });
  mkdirSync(incomplete, { recursive: true });
  const album = '@@shared\\Music\\Afterhours';
  const first = `${album}\\01 First Light.flac`;
  const second = `${album}\\02 Tape Hiss.wav`;
  const flacSource = join(MUSIC, 'clean_master.flac');
  const wavSource = join(MUSIC, 'clean_master_16bit.wav');
  const slskd = await startFakeSlskd(
    [
      { username: 'night-owl', filename: first, size: statSync(flacSource).size, bitDepth: 16, sampleRate: 44100, length: 20, source: flacSource },
      { username: 'night-owl', filename: second, size: statSync(wavSource).size, bitDepth: 16, sampleRate: 44100, length: 20, source: wavSource },
      { username: 'night-owl', filename: `${album}\\03 Radio Edit.mp3`, size: 400_000, bitRate: 320, length: 10 },
    ],
    { downloads, incomplete, rate: 24 * 1024 },
  );
  slskd.key = 'e2e-preview-key-0123456789';
  try {
    // Accepted and connected without the screens the tests before went through.
    await page.goto('/#/soulseek');
    await api(page, 'sources.accept');
    await api(page, 'slskd.configure', { url: slskd.url, apiKey: slskd.key });
    await page.reload();
    const tracksBefore = (await api<{ total: number }>(page, 'library.query', { limit: 1 })).total;

    await page.getByRole('tab', { name: 'Search' }).click();
    await page.getByRole('searchbox', { name: 'Search Soulseek' }).fill('afterhours');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.getByRole('radio', { name: 'All formats' }).click();
    const folder = page.getByRole('region', { name: 'Afterhours from night-owl' });
    await expect(folder.getByText('01 First Light.flac', { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText(/A preview plays a file while it arrives/)).toBeVisible();
    // MP3s cannot be previewed, and say why.
    await expect(folder.getByRole('button', { name: /^MP3 cannot be previewed/ })).toBeDisabled();

    // Play: the player shows the preview and plays before the file has all arrived.
    await folder.getByRole('button', { name: 'Preview 01 First Light.flac' }).click();
    const player = page.getByRole('region', { name: 'Player' });
    await expect(player.getByText('Preview', { exact: true })).toBeVisible();
    await expect(player.getByText('01 First Light.flac')).toBeVisible();
    await expect(player.getByRole('button', { name: 'Pause' })).toBeVisible({ timeout: 15_000 });
    await expect.poll(async () => Number(await player.getByRole('slider', { name: 'Position' }).getAttribute('aria-valuenow')), { timeout: 15_000 }).toBeGreaterThanOrEqual(1);
    const t1 = slskd.transfers.find((t) => t.filename === first);
    expect(t1?.bytesTransferred ?? 0).toBeLessThan(t1?.size ?? 0);
    expect(t1?.destination).toMatch(/^\.sonora-preview\//);
    // (The stand-in sends slower than it plays, so it may be buffering.)
    await expect(folder.getByRole('row', { name: /01 First Light\.flac/ })).toContainText(/Playing|Arriving as it plays|Buffering…/);

    // A preview is not a download: slskd got no download request, and the Downloads list stays empty.
    expect(slskd.transfers.filter((t) => !t.batchId)).toHaveLength(0);
    expect((await api<{ downloads: unknown[] }>(page, 'slskd.downloads')).downloads).toHaveLength(0);

    // Queue the WAV, then go on to it: the first preview stops and leaves nothing.
    await folder.getByRole('button', { name: 'Add 02 Tape Hiss.wav to the preview queue' }).click();
    const queue = page.getByRole('region', { name: 'Preview queue' });
    await expect(queue).toContainText('02 Tape Hiss.wav');
    await player.getByRole('button', { name: /^Next preview/ }).click();
    await expect(player.getByText('02 Tape Hiss.wav')).toBeVisible();
    await expect(queue).toHaveCount(0);
    await expect.poll(() => slskd.transfers.find((t) => t.filename === first)?.removed ?? false, { timeout: 15_000 }).toBe(true);
    await expect.poll(() => existsSync(incompletePath(incomplete, 'night-owl', first)), { timeout: 15_000 }).toBe(false);
    await expect(player.getByRole('button', { name: 'Pause' })).toBeVisible({ timeout: 15_000 });

    // Closing the player stops the second one too. Nothing was added to the library or kept.
    await player.getByRole('button', { name: 'Close the player' }).click();
    await expect.poll(() => slskd.transfers.filter((t) => !t.removed).length, { timeout: 15_000 }).toBe(0);
    await expect.poll(() => existsSync(incompletePath(incomplete, 'night-owl', second)), { timeout: 15_000 }).toBe(false);
    expect(existsSync(join(downloads, '.sonora-preview'))).toBe(false);
    expect((await api<{ total: number }>(page, 'library.query', { limit: 1 })).total).toBe(tracksBefore);
  } finally {
    await slskd.close();
  }
});

test('a YouTube playlist is read, searched, downloaded by the rules and checked on arrival', async ({ page }) => {
  // Searching, waiting for arrivals and a fallback take a while, all against stand-ins.
  test.setTimeout(240_000);
  // A stand-in YouTube serves the saved playlist page; a stand-in slskd
  // answers every search with the same shared files (SONORA's rules pick
  // among them) and "receives" downloads into its folder. Nothing reaches
  // YouTube or Soulseek.
  const root = join(DATA, 'playlist-slskd');
  rmSync(root, { recursive: true, force: true });
  const downloads = join(root, 'downloads');
  const incomplete = join(root, 'incomplete');
  mkdirSync(downloads, { recursive: true });
  mkdirSync(incomplete, { recursive: true });
  const ep = '@@shared\\Music\\Kerri Chandler - Rain EP';
  const nd = '@@shared\\Music\\Night Drive - Afterglow';
  const clean = join(MUSIC, 'clean_master.flac');
  const fakeMp3 = join(MUSIC, 'transcode_mp3_128.flac');
  const other = join(MUSIC, 'clean_master_16bit.wav');
  const tube = await startFakeYoutube(YOUTUBE_PORT);
  const slskd = await startFakeSlskd(
    [
      { username: 'crate-digger', filename: `${ep}\\01 Rain (Original Mix).flac`, size: statSync(clean).size, bitDepth: 16, sampleRate: 44100, length: 20, source: clean },
      // A "FLAC" made from an MP3: the first pick for Afterglow, which the arrival check throws out.
      { username: 'crate-digger', filename: `${nd}\\02 Afterglow.flac`, size: statSync(fakeMp3).size, bitDepth: 16, sampleRate: 44100, length: 20, source: fakeMp3 },
      { username: 'backup', filename: `${nd}\\Night Drive - Afterglow.wav`, size: statSync(other).size, bitDepth: 16, sampleRate: 44100, length: 20, source: other },
      { username: 'mp3-only', filename: 'mp3s\\Rain (Original Mix).mp3', size: 480_000, bitRate: 320, length: 20 },
    ],
    { downloads, incomplete, rate: 256 * 1024 },
  );
  try {
    // slskd as set up by the earlier tests (or here, so this test runs on
    // its own), with this stand-in's address and key.
    await page.goto('/#/soulseek');
    await api(page, 'sources.accept');
    slskd.key = 'e2e-playlist-key-0123456789';
    await api(page, 'slskd.configure', { url: slskd.url, apiKey: slskd.key });
    await api(page, 'sources.configure', { folder: downloads });
    await api(page, 'settings.update', { values: { playlistRules: { searchSeconds: 15, retryNotFound: false, maxDownloads: 2, maxQueue: 20, mode: 'automatic', lossy: false } } });

    await page.goto('/#/soulseek?tab=playlist');
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Download a YouTube playlist from Soulseek' })).toBeVisible();
    const link = page.getByRole('textbox', { name: 'YouTube playlist link' });

    // A private list is refused with the reason; so is a link without a playlist.
    await link.fill('https://www.youtube.com/watch?v=e2eRain0001');
    await page.getByRole('button', { name: 'Read the playlist' }).click();
    await expect(page.getByText('That link has no playlist in it', { exact: false })).toBeVisible();
    await link.fill('https://www.youtube.com/playlist?list=PLsomebodyElsesPrivateList');
    await page.getByRole('button', { name: 'Read the playlist' }).click();
    await expect(page.getByText('This playlist is private', { exact: false })).toBeVisible({ timeout: 15_000 });
    await page.getByRole('button', { name: 'Try another link' }).click();

    // The list, read as tracks; what is not a track is left out, and can be put back.
    await link.fill(`https://www.youtube.com/playlist?list=${E2E_PLAYLIST}`);
    await page.getByRole('button', { name: 'Read the playlist' }).click();
    await expect(page.getByRole('heading', { name: 'Late Night E2E' })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText('3 tracks to look for, 1 left out')).toBeVisible();
    expect(tube.requests).toEqual(['PLsomebodyElsesPrivateList', E2E_PLAYLIST]);
    const draft = page.locator('.pl-table.is-draft').first();
    await expect(draft.getByRole('row')).toHaveCount(4);
    await expect(draft.getByText('Kerri Chandler – Rain')).toBeVisible();
    await expect(draft.getByText('Original Mix', { exact: true })).toBeVisible();
    await expect(draft.getByText('NDR001')).toBeVisible();
    await page.getByText('Left out: 1 video').click();
    const short = page.getByRole('checkbox', { name: 'Look for Night Drive – Lights' });
    await short.check();
    await expect(page.getByText('4 tracks to look for')).toBeVisible();
    await short.uncheck();
    await expect(page.getByText('3 tracks to look for, 1 left out')).toBeVisible();
    // The quick rules are the settings themselves.
    await page.getByRole('radio', { name: 'Review first' }).click();
    await expect.poll(async () => ((await api<{ playlistRules: { mode: string } }>(page, 'settings.get')).playlistRules.mode)).toBe('review');
    await page.getByRole('radio', { name: 'Automatic' }).click();

    // Start: one search at a time, the title alone first; the pick is downloaded as soon as a track is found.
    await page.getByRole('button', { name: 'Start' }).click();
    await expect(page.locator('.pl-state')).toContainText('Searching for', { timeout: 15_000 });
    // B108: the sidebar's 30 px header style no longer squeezes the run's header (its lines ran over the counts).
    const head = await page.locator('.pl-view .pl-head').boundingBox();
    const counts = await page.locator('.pl-counts').boundingBox();
    expect(head && counts && head.y + head.height <= counts.y).toBe(true);
    expect(head?.height ?? 0).toBeGreaterThan(30);
    await expect(page.getByRole('status').filter({ hasText: 'Playlist' })).toBeVisible();
    await expect(page.locator('.pl-state')).toContainText('Done: 2 tracks downloaded and checked, 1 not found.', { timeout: 200_000 });
    expect(slskd.searches[0]).toBe('Rain');
    // The track no one has is looked for down to the artist alone.
    expect(slskd.searches).toContain('Someone');
    expect(slskd.searches).toContain('Someone Nowhere To Be Found');
    await expect(page.getByRole('status').filter({ hasText: 'Playlist' })).toHaveCount(0);

    // Rain: found with the first search, downloaded, checked; the MP3 was out by the rules.
    const rain = page.getByRole('row', { name: /Kerri Chandler – Rain/ }).first();
    await expect(rain).toContainText('Done');
    await expect(rain).toContainText('01 Rain (Original Mix).flac');
    await rain.getByRole('button', { name: 'Details of Kerri Chandler – Rain' }).click();
    const rainDetail = page.locator('.pl-detail').first();
    await expect(rainDetail).toContainText('Found by searching for the title.');
    await expect(rainDetail).toContainText('MP3: lossy formats are off');
    await expect(rainDetail.getByText('Downloaded')).toBeVisible();

    // Afterglow: the first pick turned out to be made from an MP3, so the next file was taken.
    const after = page.getByRole('row', { name: /Night Drive – Afterglow/ }).first();
    await expect(after).toContainText('Done');
    await expect(after).toContainText('Night Drive - Afterglow.wav');
    await after.getByRole('button', { name: 'Details of Night Drive – Afterglow' }).click();
    const afterDetail = page.locator('.pl-detail').nth(1);
    await expect(afterDetail).toContainText('02 Afterglow.flac from crate-digger did not work out (SONORA found signs it was made from a lossy source');
    await expect(afterDetail).toContainText('2 download tries');

    // Nowhere To Be Found: everyone answered with other files, all rejected with a reason.
    await page.getByRole('button', { name: 'Not found' }).click();
    const none = page.getByRole('row', { name: /Someone – Nowhere To Be Found/ }).first();
    await expect(none).toContainText('Every file found failed the rules.');
    await none.getByRole('button', { name: 'Details of Someone – Nowhere To Be Found' }).click();
    await expect(page.locator('.pl-detail').first()).toContainText('shares only 0% of the title’s words');
    await page.getByRole('button', { name: /^All \d/ }).click();

    // The library has the two files, checked: the arrival check is the usual one.
    await waitIdle(page);
    const tracks = await api<{ total: number; rows: { fileName: string; flags: string[] }[] }>(page, 'library.query', { filter: {}, limit: 500 });
    const names = tracks.rows.map((r) => r.fileName);
    expect(names).toContain('01 Rain (Original Mix).flac');
    expect(names).toContain('Night Drive - Afterglow.wav');
    expect(names).toContain('02 Afterglow.flac');
    expect(tracks.rows.find((r) => r.fileName === '02 Afterglow.flac')?.flags).toContain('source_concern');

    // Settings holds every rule.
    await page.goto('/#/settings');
    const rules = page.getByRole('region', { name: 'Playlist download rules' });
    await expect(rules.getByRole('checkbox', { name: /^FLAC/ })).toBeChecked();
    await expect(rules.getByRole('checkbox', { name: /^WavPack/ })).not.toBeChecked();
    const bitrate = rules.getByRole('textbox', { name: 'Lowest bit rate', exact: true });
    await expect(bitrate).toHaveCount(0);
    await rules.getByRole('switch', { name: 'Lossy files too' }).click();
    await expect(bitrate).toHaveValue('320');
    await rules.getByRole('switch', { name: 'Lossy files too' }).click();
    await expect(bitrate).toHaveCount(0);
    await page.reload();
    await expect(page.getByRole('region', { name: 'Playlist download rules' }).getByRole('switch', { name: 'Lossy files too' })).not.toBeChecked();
  } finally {
    await slskd.close();
    await tube.close();
  }
});

// ------------------------------------------------------------------
// Regression tests for the 0.3.2 review (docs/BUGS.md). Each makes what
// it needs and takes it away again.

test('review: the library keeps the selection through a search, Clear beats a search being typed, and the keyboard stays put (B24–B26, B43, B62–B64, B67)', async ({ page }) => {
  await page.goto('/#/library');
  await expect(rows(page).first()).toBeVisible();
  const total = await rows(page).count();
  const search = page.getByRole('searchbox', { name: 'Search tracks' });
  const panel = page.getByRole('complementary', { name: 'Track details' });

  // B24: a search keeps a selected track that is still in the results.
  const first = rows(page).first();
  const title = (await first.locator('.cell-title').innerText()).trim();
  await first.click();
  await expect(first).toHaveAttribute('aria-selected', 'true');
  await search.fill(title);
  await expect(rows(page)).not.toHaveCount(total);
  await expect(row(page, title)).toHaveAttribute('aria-selected', 'true');
  await expect(panel.getByRole('heading', { level: 2 })).toHaveText(title);

  // B25: Clear right after typing: what was being typed does not apply later.
  // B62: and the keyboard goes to the search field.
  await search.fill('zzzz nothing here');
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(search).toBeFocused();
  await page.waitForTimeout(800);
  await expect(search).toHaveValue('');
  await expect(rows(page)).toHaveCount(total);

  // B26: ▶ on a row leaves the keyboard with the table.
  await rows(page).nth(1).click();
  await rows(page).nth(3).locator('.row-play').click();
  expect(await page.evaluate(() => !!document.activeElement?.closest('[role="grid"]') && !document.activeElement?.classList.contains('row-play'))).toBe(true);
  await page.getByRole('button', { name: 'Close the player' }).click();

  // B63: a row menu closed by scrolling gives the keyboard back to the table.
  await rows(page).nth(1).click();
  await page.keyboard.press('Shift+F10');
  const rowMenu = page.getByRole('menu');
  await expect(rowMenu).toBeVisible();
  await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('[role="menu"]'))).toBe(true);
  await rows(page).nth(5).hover();
  await page.mouse.wheel(0, 400);
  await expect(rowMenu).toBeHidden();
  expect(await page.evaluate(() => !!document.activeElement?.closest('[role="grid"]'))).toBe(true);

  // B64: ↑ in a menu opened with the mouse goes to the last item.
  await page.getByRole('toolbar', { name: 'Filters' }).getByRole('button', { name: /^Issues/ }).click();
  const issues = page.getByRole('menu', { name: 'Issue filter' });
  await expect(issues).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(issues.getByRole('menuitemcheckbox').last()).toBeFocused();
  await page.keyboard.press('Escape');

  // B43: emptying a field that holds no edit changes nothing, and says what stays.
  await search.fill('other_track');
  await expect(rows(page)).toHaveCount(1);
  await row(page, 'other_track').click();
  await panel.getByRole('button', { name: 'Edit…' }).click();
  const dj = page.getByRole('dialog', { name: 'Edit BPM, key and genre' });
  const tempo = dj.getByRole('textbox', { name: /^Tempo/ });
  await tempo.fill('');
  await expect(tempo).toHaveAttribute('placeholder', 'Keeps 128');
  await expect(dj.getByRole('button', { name: 'Save' })).toBeDisabled();
  await dj.getByRole('button', { name: 'Cancel' }).click();
  await expect(dj).toBeHidden();

  // B67: a track that was never analysed has no cut-off "detected".
  await page.goto('/#/attention');
  await page.getByRole('tab', { name: /^Failed/ }).click();
  await rows(page).first().click();
  await expect(panel.locator('dt', { hasText: /^Cutoff$/ }).locator('xpath=following-sibling::dd[1]')).toHaveText('—');
});

test('review: a failed re-analysis says so above the earlier results (B27)', async ({ page }) => {
  const dir = join(DATA, 'review-b27');
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const file = join(dir, 'b27_track.flac');
  copyFileSync(join(MUSIC, 'clean_master.flac'), file);
  await page.goto('/#/library');
  await api(page, 'library.import', { paths: [dir] });
  await waitIdle(page);
  const found = await api<{ rows: { id: number; status: string }[] }>(page, 'library.query', { filter: { search: 'b27_track' }, sort: { key: 'title', desc: false }, offset: 0, limit: 5 });
  const id = found.rows[0]?.id ?? 0;
  expect(found.rows[0]?.status).toBe('done');
  try {
    // The file is replaced by something that is not audio, then analysed again.
    writeFileSync(file, Buffer.alloc(4096, 7));
    await api(page, 'tracks.rescan', { ids: [id] });
    await waitIdle(page);
    await page.goto(`/#/track/${id}`);
    const notice = page.locator('.earlier-results');
    await expect(notice).toContainText('The latest analysis failed');
    await expect(notice).toContainText('The results below are from an earlier analysis.');
    await expect(notice.getByRole('button', { name: 'Try again' })).toBeVisible();
  } finally {
    const folders = await api<{ id: number; path: string }[] | { folders: { id: number; path: string }[] }>(page, 'folders.list');
    const list = Array.isArray(folders) ? folders : folders.folders;
    const mine = list.find((f) => f.path === dir);
    if (mine) await api(page, 'folders.remove', { id: mine.id, removeTracks: true });
    rmSync(dir, { recursive: true, force: true });
  }
});

test('review: smart playlists keep their rules as saved, and their selection and rows follow the tracks (B40–B42, B94, B95)', async ({ page }) => {
  await page.goto('/#/library');
  await expect(rows(page).first()).toBeVisible();
  const made: number[] = [];
  const create = async (kind: 'smart' | 'playlist', name: string, extra: Record<string, unknown>) => {
    const n = await api<{ id: number }>(page, 'playlists.create', { kind, name, ...extra });
    made.push(n.id);
    return n.id;
  };
  const filterOf = async (id: number) =>
    (await api<{ nodes: { id: number; rules: { filter: Record<string, unknown> } | null }[] }>(page, 'playlists.list')).nodes.find((n) => n.id === id)?.rules?.filter;
  const sort = { key: 'bpm', desc: false };
  let slowCounts = false;
  let slowTracks = false;
  await page.route('**/api/call', async (route) => {
    const b = route.request().postDataJSON() as { method?: string; params?: { limit?: number } };
    if ((slowCounts && b.method === 'library.query' && b.params?.limit === 1) || (slowTracks && b.method === 'playlists.tracks')) await new Promise((r) => setTimeout(r, 2500));
    return route.continue();
  });
  try {
    const wide = await create('smart', 'Review wide', { rules: { filter: { bpmMin: 60, bpmMax: 200 }, sort } });
    const genre = await create('smart', 'Review genre', { rules: { filter: { genres: ['Zydeco Polka'] }, sort } });
    const tree = page.getByRole('tree', { name: 'Playlists' });
    const edit = page.getByRole('dialog', { name: 'Edit smart playlist' });

    // B42: a tempo range saved from the library is kept as it is when the rules are saved unchanged.
    await tree.getByRole('treeitem', { name: /Review wide/ }).click();
    await expect(rows(page).first()).toBeVisible();
    await page.getByRole('button', { name: 'Edit rules…' }).click();
    await expect(edit.getByRole('textbox', { name: 'Highest BPM' })).toHaveValue('200');
    const counted = edit.getByText(/^\d+ tracks? match(es)? now$/);
    await expect(counted).toBeVisible();
    const wideCount = await counted.innerText();
    expect(wideCount).not.toBe('0 tracks match now');
    await edit.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(edit).toBeHidden();
    expect((await filterOf(wide))?.bpmMax).toBe(200);

    // B94: the next playlist's rules do not open with this one's count.
    await tree.getByRole('treeitem', { name: /Review genre/ }).click();
    slowCounts = true;
    await page.getByRole('button', { name: 'Edit rules…' }).click();
    await expect(edit).toBeVisible();
    expect(await edit.innerText()).not.toContain(wideCount);
    slowCounts = false;
    // B41: a genre the library does not list among its most common still shows, and can be turned off.
    const chip = edit.getByRole('group', { name: 'Genres' }).getByRole('button', { name: /Zydeco Polka/ });
    await expect(chip).toHaveAttribute('aria-pressed', 'true');
    await expect(edit).toContainText('Genres (Zydeco Polka)');
    await chip.click();
    await expect(chip).toHaveCount(0);
    await edit.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(edit).toBeHidden();
    const saved = await filterOf(genre);
    expect(saved).toBeTruthy();
    expect(saved?.genres).toBeUndefined();

    // B40: tracks that leave a smart playlist leave its selection.
    await tree.getByRole('treeitem', { name: /Review wide/ }).click();
    await rows(page).nth(0).click();
    await rows(page).nth(1).click({ modifiers: ['Shift'] });
    const selected = page.getByText('2 tracks selected');
    await expect(selected).toBeVisible();
    await api(page, 'playlists.update', { id: wide, rules: { filter: { bpmMin: 390, bpmMax: 400 }, sort } });
    await expect(rows(page)).toHaveCount(0);
    await expect(selected).toBeHidden();

    // B95: tracks taken out of a playlist go from the screen at once.
    const ids = (await api<{ ids: number[] }>(page, 'library.ids', { filter: {} })).ids.slice(0, 3);
    const plain = await create('playlist', 'Review plain', { trackIds: ids });
    await tree.getByRole('treeitem', { name: /Review plain/ }).click();
    await expect(rows(page)).toHaveCount(3);
    slowTracks = true;
    await rows(page).first().click();
    await page.keyboard.press('Delete');
    await expect(rows(page)).toHaveCount(2, { timeout: 1500 });
    slowTracks = false;
    await expect.poll(async () => (await api<{ total: number }>(page, 'playlists.tracks', { id: plain })).total).toBe(2);
  } finally {
    slowCounts = false;
    slowTracks = false;
    await page.unroute('**/api/call');
    for (const id of made.reverse()) await api(page, 'playlists.delete', { id }).catch(() => undefined);
  }
});

test('review: the USB dialog plans again when another job ends and says plainly when a send fails; settings steppers and rules read right (B09, B46, B51, B57, B86, B103)', async ({ page }) => {
  await page.addInitScript(() => {
    const Original = window.EventSource;
    const streams: EventSource[] = [];
    (window as unknown as { streams: EventSource[] }).streams = streams;
    window.EventSource = class extends Original {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        streams.push(this);
      }
    };
  });
  await page.goto('/#/library');
  await expect(rows(page).first()).toBeVisible();
  const ids = (await api<{ ids: number[] }>(page, 'library.ids', { filter: {} })).ids.slice(0, 2);
  const list = await api<{ id: number }>(page, 'playlists.create', { kind: 'playlist', name: 'Review USB', trackIds: ids });
  const usb = join(DATA, 'review-usb');
  rmSync(usb, { recursive: true, force: true });
  mkdirSync(usb, { recursive: true });
  // The core says an export runs until the test says otherwise.
  let busy = true;
  await page.route('**/api/call', async (route) => {
    const b = route.request().postDataJSON() as { method?: string };
    if (b.method !== 'usb.plan' || !busy) return route.continue();
    const res = await route.fetch();
    const j = (await res.json()) as { ok: boolean; result?: { running?: boolean } };
    if (j.ok && j.result) j.result.running = true;
    return route.fulfill({ response: res, json: j });
  });
  try {
    await page.getByRole('tree', { name: 'Playlists' }).getByRole('treeitem', { name: /Review USB/ }).click();
    await page.getByRole('button', { name: 'Send to USB…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Send playlists to a USB drive' });
    await dialog.getByRole('textbox', { name: 'Drive or folder (full path)' }).fill(usb);
    const waiting = dialog.getByText('An export or another send is running');
    await expect(waiting).toBeVisible();
    // B09: the export ends: the dialog asks again and Send can be used.
    busy = false;
    await page.waitForFunction(() => (window as unknown as { streams: EventSource[] }).streams.some((s) => s.readyState === 1));
    await page.evaluate(() => {
      for (const s of (window as unknown as { streams: EventSource[] }).streams) {
        s.dispatchEvent(new MessageEvent('djexport.finished', { data: JSON.stringify({ folder: '/nowhere', written: [], failed: [], cancelled: true }) }));
      }
    });
    await expect(waiting).toBeHidden();
    await expect(dialog.getByRole('button', { name: 'Send to review-usb' })).toBeEnabled();

    // B51: a send that fails says so in one sentence, and does not explain
    // how to use a drive that holds nothing. (The send is played: the core
    // is asked to start nothing.)
    await page.route('**/api/call', async (route) => {
      const b = route.request().postDataJSON() as { method?: string };
      if (b.method === 'usb.start') return route.fulfill({ json: { ok: true, result: { accepted: true } } });
      return route.fallback();
    });
    await dialog.getByRole('button', { name: 'Send to review-usb' }).click();
    await page.evaluate((folder) => {
      const report = { root: folder, folder, playlists: 0, written: 0, renamed: 0, kept: 0, removed: 0, bytes: 0, failed: [], m3u: 0, crates: 0, rekordboxXml: null, warnings: [], cancelled: false, elapsedMs: 10, error: 'The drive went away. The details are in the log.' };
      for (const s of (window as unknown as { streams: EventSource[] }).streams) s.dispatchEvent(new MessageEvent('usb.finished', { data: JSON.stringify(report) }));
    }, usb);
    const failed = page.getByRole('dialog').filter({ hasText: 'Sending stopped' });
    await expect(failed).toContainText('The drive went away. The details are in the log.');
    expect(await failed.innerText()).not.toContain('log..');
    await expect(failed.getByRole('heading', { name: 'Using the drive' })).toHaveCount(0);
    await failed.getByRole('button', { name: 'Done' }).click();
  } finally {
    busy = false;
    await page.unroute('**/api/call');
    await api(page, 'playlists.delete', { id: list.id });
    rmSync(usb, { recursive: true, force: true });
  }

  const before = await api<{ workers: number; maxDurationMin: number; playlistRules: Record<string, unknown> }>(page, 'settings.get');
  try {
    await api(page, 'settings.update', { values: { playlistRules: { ...before.playlistRules, maxBitDepth: 24, maxSampleRate: 96000 } } });
    // Read again by the page (a change of address alone keeps what it read).
    await page.goto('/#/settings');
    await page.reload();
    // B57: with units.
    await expect(page.getByText('Files above this are left out: 24-bit, 96 kHz.')).toBeVisible();
    // B86: + steps from what was typed, not from the value saved before.
    // B103: and the two saves (the typed value, then +) arrive in that order,
    // even when the first is slow to get there.
    const typed = before.workers === 3 ? 5 : 3;
    await page.route('**/api/call', async (route) => {
      const body = route.request().postDataJSON() as { method?: string; params?: { values?: { workers?: number } } };
      if (body.method === 'settings.update' && body.params?.values?.workers === typed) await new Promise((r) => setTimeout(r, 800));
      return route.continue();
    });
    const workers = page.getByRole('textbox', { name: 'Parallel analyses' });
    await workers.fill(String(typed));
    await page.getByRole('button', { name: 'Increase Parallel analyses' }).click();
    await expect(workers).toHaveValue(String(typed + 1));
    await expect.poll(async () => (await api<{ workers: number }>(page, 'settings.get')).workers).toBe(typed + 1);
    await page.waitForTimeout(1000);
    expect((await api<{ workers: number }>(page, 'settings.get')).workers).toBe(typed + 1);
    await expect(workers).toHaveValue(String(typed + 1));
    await page.unroute('**/api/call');
    // B46: an emptied field goes back to the saved value, not to its minimum.
    const longest = page.getByRole('textbox', { name: 'Longest file in minutes' });
    await longest.fill('');
    await longest.press('Tab');
    await expect(longest).toHaveValue(String(before.maxDurationMin));
    expect((await api<{ maxDurationMin: number }>(page, 'settings.get')).maxDurationMin).toBe(before.maxDurationMin);
  } finally {
    await api(page, 'settings.update', { values: { workers: before.workers, playlistRules: before.playlistRules } });
  }
});
