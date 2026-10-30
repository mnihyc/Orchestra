/** A readable thread's redacted parents do not replace the person's chosen global navigation. */
import { expect, test } from '@playwright/test';
import { begin, MAIN, openThread } from './inspector-support.ts';
import { ACME, emit } from './support.ts';

for (const theme of ['light', 'dark'] as const) test(`a private thread parent preserves the chosen navigation in ${theme}`, async ({ page }, info) => {
  await begin(); await page.setViewportSize({ width: 1440, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  await openThread(page);
  const left = page.locator('[data-workbench-bar="left"]:visible');
  await expect(left.getByRole('searchbox', { name: 'Narrow this tree', exact: true })).toBeVisible();
  const remembered = await page.evaluate(() => Object.entries(localStorage).filter(([key]) => /^orch\..+\.org$/.test(key)));
  expect(remembered.some(([, value]) => value === ACME.org)).toBe(true);
  const hidden = '00000000-0000-4000-8001-00000000f001', reads: string[] = [], errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname.includes(hidden)) reads.push(request.url()); });
  await emit(MAIN, { thread: { org_id: hidden, path: [
    { id: hidden, kind: 'org', name: null, redacted: true },
    { id: ACME.fixLogin, kind: 'topic', name: null, redacted: true },
    { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
  ] } });
  await expect(page.locator('[data-screen-path]')).toContainText('Private org');
  await expect(left.getByRole('searchbox', { name: 'Narrow this tree', exact: true })).toBeVisible();
  expect(await page.evaluate(() => Object.entries(localStorage).filter(([key]) => /^orch\..+\.org$/.test(key)))).toEqual(remembered);
  expect(reads).toEqual([]);
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    if (width === 400) await page.locator('[data-container-choice="left:navigation"]').click();
    await expect(left.getByRole('searchbox', { name: 'Narrow this tree', exact: true })).toBeVisible();
    await page.mouse.move(0, 0); await page.evaluate(() => document.fonts.ready);
    const path = info.outputPath(`private-thread-navigation-${width}-${theme}.png`);
    await page.screenshot({ path, fullPage: true, animations: 'disabled' });
    await info.attach(`navigation-${width}`, { path, contentType: 'image/png' });
  }
  expect(reads).toEqual([]); expect(errors).toEqual([]);
});
