/**
 * End-to-end tests of the SONORA interface against the real analysis core.
 *
 * The tests run in order on one library: the first imports the generated
 * corpus through the interface, the rest work with the analyzed tracks.
 */

import { expect, test, type Page } from '@playwright/test';
import { copyFileSync, existsSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { MUSIC } from './paths.ts';

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

function rows(page: Page) {
  return page.locator('.tbody .trow:not(.is-placeholder)');
}

function row(page: Page, title: string) {
  return page.locator('.tbody .trow').filter({ has: page.locator('.cell-title', { hasText: new RegExp(`^${title}$`) }) });
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
  for (const r of await rows(page).all()) await expect(r.locator('.chip-red', { hasText: 'Clipping' })).toBeVisible();
  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(rows(page)).toHaveCount(total);

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
  await expect(dialog).toContainText('nothing on disk is deleted');
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

test('rescanning a folder picks up new files and skips unchanged ones', async ({ page }) => {
  const extra = join(MUSIC, 'zz_new_copy.flac');
  rmSync(extra, { force: true });
  copyFileSync(join(MUSIC, 'other_track.flac'), extra);
  try {
    await page.goto('/#/folders');
    await expect(page.getByRole('cell', { name: /music/ }).first()).toBeVisible();
    await page.getByRole('button', { name: 'Rescan this folder' }).first().click();
    await waitIdle(page);
    await page.goto('/#/library');
    await page.getByRole('searchbox', { name: 'Search tracks' }).fill('zz_new_copy');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('.score-num').first()).not.toHaveText('—');
    // Unchanged files are skipped: the scan analyzed only the new file and
    // the one removed from the library earlier, which a rescan brings back.
    await page.goto('/#/history');
    const latest = page.locator('.history-table tbody tr').first();
    await expect(latest).toContainText('Folder rescan');
    await expect(latest.locator('td').nth(3)).toHaveText('2');
  } finally {
    rmSync(extra, { force: true });
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
  await expect(page.getByText('SONORA works entirely on this computer')).toBeVisible();
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
