import { expect, test } from '@playwright/test';

// Exploration only (not for merging): when WebKit lets a page write to the
// clipboard. Fails on purpose, with what it found in the failure message.
test('explore: the clipboard in WebKit', async ({ page }) => {
  await page.goto('/');
  const ids = ['sync-writeText', 'fetch-then-writeText', 'slow-then-writeText', 'item-fetch', 'item-slow'];
  await page.evaluate((ids) => {
    const w = window as unknown as { r: Record<string, string> };
    w.r = { hasItem: typeof ClipboardItem, hasWrite: typeof navigator.clipboard?.write };
    const wait = (ms: number) => new Promise((res) => setTimeout(res, ms));
    const blob = (t: string) => new Blob([t], { type: 'text/plain' });
    const runs: Record<string, () => Promise<unknown>> = {
      'sync-writeText': () => navigator.clipboard.writeText('a'),
      'fetch-then-writeText': async () => {
        await fetch('/api/health');
        await navigator.clipboard.writeText('b');
      },
      'slow-then-writeText': async () => {
        await wait(1500);
        await navigator.clipboard.writeText('c');
      },
      'item-fetch': () => navigator.clipboard.write([new ClipboardItem({ 'text/plain': fetch('/api/health').then(() => blob('d')) })]),
      'item-slow': () => navigator.clipboard.write([new ClipboardItem({ 'text/plain': wait(1500).then(() => blob('e')) })]),
    };
    ids.forEach((id, i) => {
      const b = document.createElement('button');
      b.id = id;
      b.textContent = id;
      b.style.cssText = `position:fixed;top:${10 + i * 44}px;right:10px;z-index:99999;padding:8px`;
      b.addEventListener('click', () => {
        void runs[id]!().then(
          () => (w.r[id] = 'ok'),
          (e: unknown) => (w.r[id] = e instanceof Error ? `${e.name}: ${e.message}` : String(e)),
        );
      });
      document.body.append(b);
    });
  }, ids);
  for (const id of ids) {
    await page.locator(`#${id}`).click();
    await page.waitForTimeout(2500);
  }
  const noGesture = await page.evaluate(() => navigator.clipboard.writeText('f').then(() => 'ok', (e: Error) => `${e.name}: ${e.message}`));
  const r = await page.evaluate(() => (window as unknown as { r: Record<string, string> }).r);
  expect(JSON.stringify({ ...r, noGesture }, null, 1)).toBe('explore');
});
