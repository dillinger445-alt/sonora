import { expect, test } from '@playwright/test';

/**
 * The macOS app shows SONORA's interface in WebKit, Safari's engine. These
 * checks run there (after the main suite, in the state it leaves: slskd set
 * up by SONORA), for what WebKit lays out differently from Chromium.
 */

test('dialogs open with their content in WebKit', async ({ page }) => {
  // Every dialog is the same component; this one asks before slskd is turned off.
  await page.goto('/#/soulseek');
  await page.getByRole('tab', { name: 'Setup' }).click();
  await page.getByRole('button', { name: 'Turn off' }).click();
  const dialog = page.getByRole('dialog', { name: 'Turn slskd off?' });
  await expect(dialog).toBeVisible();
  // Until 0.3.2, WebKit gave the box no height: the window only darkened.
  const box = await dialog.boundingBox();
  expect(box?.height ?? 0).toBeGreaterThan(120);
  await expect(dialog.getByText('slskd stops and no longer starts when you log in')).toBeInViewport();
  // A click reaches its buttons, and Cancel changes nothing.
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('button', { name: 'Turn off' })).toBeEnabled();
});
