/** The explorer's actual controls, scrolling and idle wire behavior on production assets. */
import { writeFile } from 'node:fs/promises';
import { expect, type Page, type TestInfo, test } from '@playwright/test';
import { begin, fileTree, filesPanel, MAIN, MAIN_ENVIRONMENT, onDisk, openThread, row, showView, workspace } from './inspector-support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';

async function capture(page: Page, info: TestInfo, name: string) {
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.fonts.ready);
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path, fullPage: true, animations: 'disabled' });
  await info.attach(name, { path, contentType: 'image/png' });
}
test.beforeEach(async () => { await begin(); });

for (const [width, theme] of [[1440, 'light'], [400, 'dark']] as const) test(`directory failure and invalidation keep rows and recover explicitly at ${width}/${theme}`, async ({ page }, info) => {
  await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  const reads: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() === 'GET' && url.pathname === `/api/v1/environments/${MAIN_ENVIRONMENT}/files`) reads.push(url.search);
  });
  await openThread(page); await showView(page, /^Files/);
  const panel = filesPanel(page);
  await expect(row(page, /^src/)).toBeVisible();
  await expect(panel.getByText('Live', { exact: true })).toBeVisible();
  await workspace('/directory-event', { path: 'web', kind: 'failure' });
  await expect(panel.getByText('The directory watch could not be installed.', { exact: true })).toBeVisible();
  await expect(row(page, /^src/)).toBeVisible();
  await expect(panel.getByText('Live', { exact: true })).toHaveCount(0);
  await capture(page, info, `watch-failed-${width}-${theme}`);
  const failedReads = reads.length;
  await workspace('/put', { files: { 'web/after-failure.txt': 'available on retry' } });
  await expect(row(page, /^after-failure/)).toHaveCount(0);
  expect(reads).toHaveLength(failedReads);
  await panel.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(row(page, /^after-failure/)).toBeVisible();
  await expect(panel.getByText('Live', { exact: true })).toBeVisible();
  await expect.poll(() => reads.length).toBe(failedReads + 1);
  await workspace('/directory-event', { path: 'web', kind: 'changed' });
  await expect(panel.getByText('This directory changed.', { exact: true })).toBeVisible();
  await expect(row(page, /^src/)).toBeVisible();
  await expect(panel.getByText('Live', { exact: true })).toHaveCount(0);
  const staleReads = reads.length;
  await capture(page, info, `watch-stale-${width}-${theme}`);
  expect(reads).toHaveLength(staleReads);
  await panel.getByRole('button', { name: 'Reload directory', exact: true }).click();
  await expect(panel.getByText('This directory changed.', { exact: true })).toHaveCount(0);
  await expect(panel.getByText('Live', { exact: true })).toBeVisible();
  await expect.poll(() => reads.length).toBe(staleReads + 1);
});

for (const theme of ['light', 'dark'] as const) test(`the explorer retains compact rows and reachable named actions in ${theme}`, async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: theme });
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors: string[] = [], reads: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => { const url = new URL(request.url()); if (request.method() === 'GET' && /\/files(?:\?|$)/.test(url.pathname + url.search)) reads.push(url.pathname + url.search); });
  await openThread(page); await showView(page, /^Files/);
  const panel = filesPanel(page);
  await expect(fileTree(page)).toBeVisible();
  await expect(panel.getByRole('textbox', { name: 'Search the files' })).toHaveCount(0);
  await row(page, /^src/).focus(); await page.keyboard.press('ArrowRight');
  await expect(row(page, /^app\.ts/)).toBeVisible();
  await page.keyboard.press('ArrowRight'); await expect(row(page, /^app\.ts/)).toBeFocused();
  await page.keyboard.press('Shift+F10');
  await expect(page.getByRole('menuitem', { name: 'Rename…', exact: true })).toBeVisible();
  await capture(page, info, `explorer-context-${theme}`); await page.keyboard.press('Escape');
  await panel.getByRole('button', { name: 'Menu of the tree', exact: true }).click();
  for (const name of ['New file…', 'New folder…', 'Refresh', 'Collapse all', 'Search the files']) await expect(page.getByRole('menuitem', { name, exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await panel.getByRole('button', { name: 'Search the files', exact: true }).click();
  await expect(panel.getByRole('textbox', { name: 'Search the files' })).toBeFocused();
  await panel.getByRole('textbox', { name: 'Search the files' }).fill('util');
  await expect(page.getByTestId('search-results').getByRole('button', { name: 'web/src/util.ts' })).toBeVisible();
  await panel.getByRole('button', { name: 'Close file search', exact: true }).click();
  await expect(row(page, /^app\.ts/)).toBeVisible();
  await capture(page, info, `explorer-1440-${theme}`);
  const beforeIdle = reads.length;
  // Observe one idle minute, the product's request bar; no clock or mocked timers.
  await page.waitForTimeout(60_000);
  expect(reads.slice(beforeIdle)).toEqual([]);
  await writeFile(info.outputPath('directory-reads.json'), JSON.stringify(reads, null, 2));
  await page.setViewportSize({ width: 400, height: 900 }); await showView(page, /^Files/);
  await expect(fileTree(page)).toBeVisible();
  await capture(page, info, `explorer-400-${theme}`);
  expect(errors).toEqual([]);
});

test('a thousand sibling rows stay scrollable and a live addition keeps unrelated branches open', async ({ page }, info) => {
  test.setTimeout(60_000);
  await page.setViewportSize({ width: 1440, height: 900 });
  await workspace('/put', { files: Object.fromEntries(Array.from({ length: 1000 }, (_, index) => [`web/wide/file-${String(index).padStart(4, '0')}.txt`, 'content'])) });
  await openThread(page); await showView(page, /^Files/);
  await row(page, /^src/).click(); await expect(row(page, /^app\.ts/)).toBeVisible();
  await row(page, /^wide/).click(); await expect(row(page, /^file-0000/)).toBeVisible();
  expect(await fileTree(page).getByRole('treeitem').count()).toBeLessThan(100);
  await row(page, /^file-0000/).focus(); await page.keyboard.press('End');
  await expect(row(page, /^README\.md/)).toBeFocused();
  await page.keyboard.type('file-0999'); await expect(row(page, /^file-0999/)).toBeFocused();
  await capture(page, info, 'explorer-wide-end');
  await workspace('/put', { files: { 'web/src/live-added.ts': 'export const live = true;' } });
  await page.keyboard.press('Home'); await expect(row(page, /^src/)).toBeFocused();
  await expect(row(page, /^live-added/)).toBeVisible();
  await expect(row(page, /^wide/)).toHaveAttribute('aria-expanded', 'true');
});

for (const warm of [false, true]) test(`a dirty file retains its original workspace after a live rebind with ${warm ? 'warm' : 'cold'} destination caches`, async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const original = await onDisk('web/src/app.ts');
  if (original === null) throw new Error('The original file is required');
  const other = RUNS_ON.docsCheckout;
  await workspace('/environment', { environment: other, patch: { status: 'running', remote_write: true, machine_online: true } });
  await workspace('/put', { environment: other, files: { 'web/src/app.ts': original } });
  if (warm) await workspace('/bind-thread', { environment: other, thread: MAIN, directory: 'web' });
  await openThread(page); await showView(page, /^Files/);
  await row(page, /^src/).click(); await row(page, /^app\.ts/).click();
  const code = page.getByRole('textbox', { name: 'Text of app.ts', exact: true });
  await expect(code).toContainText("import { greet } from './util';");
  if (warm) {
    await workspace('/bind-thread', { environment: MAIN_ENVIRONMENT, thread: MAIN, directory: 'web' });
    await expect(filesPanel(page).getByRole('heading', { name: 'web/ in Workspace', exact: true })).toBeVisible();
    await row(page, /^src/).click(); await row(page, /^app\.ts/).click();
    await expect(code).toContainText("import { greet } from './util';");
  }
  await code.click(); await page.keyboard.press('Control+Home'); await page.keyboard.type('// retained in original workspace\n');
  const writes: string[] = [];
  page.on('request', (request) => { if (request.method() === 'PUT' && new URL(request.url()).pathname.endsWith('/files/content')) writes.push(new URL(request.url()).pathname); });
  await workspace('/bind-thread', { environment: other, thread: MAIN, directory: 'web' });
  await expect(page.getByText('This file remains in Workspace, its original workspace.', { exact: true })).toBeVisible();
  await expect(filesPanel(page).getByTestId('why-read-only')).toHaveCount(0);
  await expect(code).toContainText('// retained in original workspace');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => onDisk('web/src/app.ts')).toBe(`// retained in original workspace\n${original}`);
  expect((await workspace('/content', { environment: other, path: 'web/src/app.ts' }) as { text: string }).text).toBe(original);
  expect(writes).toEqual([`/api/v1/environments/${MAIN_ENVIRONMENT}/files/content`]);
  const collisions = await page.locator('[data-composer-controls] [data-composer-control]').evaluateAll((elements) => {
    const controls = elements.map((element) => ({ name: element.getAttribute('data-composer-control'), box: element.getBoundingClientRect() })).filter(({ box }) => box.width > 0 && box.height > 0);
    const overlaps: string[] = [];
    for (let one = 0; one < controls.length; one++) for (let two = one + 1; two < controls.length; two++) {
      const a = controls[one], b = controls[two]; if (a === undefined || b === undefined) continue;
      if (Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left) > 1 && Math.min(a.box.bottom, b.box.bottom) - Math.max(a.box.top, b.box.top) > 1) overlaps.push(`${a.name}/${b.name}`);
    }
    return overlaps;
  });
  expect(collisions).toEqual([]);
  await capture(page, info, `kept-file-original-${warm ? 'warm' : 'cold'}`);
});

test('copy, cut and paste keys use the same focused paths as the named file menus', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const original = await onDisk('web/src/app.ts');
  await workspace('/put', { files: { 'web/target/.keep': '', 'web/second/.keep': '' } });
  await openThread(page); await showView(page, /^Files/);
  await row(page, /^src/).click();
  await fileTree(page).locator('[data-path="web/src/app.ts"]').focus(); await page.keyboard.press('Control+c');
  await row(page, /^target/).click(); await page.keyboard.press('Control+v');
  await expect.poll(() => onDisk('web/target/app.ts')).toBe(original);
  const copied = fileTree(page).locator('[data-path="web/target/app.ts"]'); await copied.focus(); await page.keyboard.press('Control+x');
  await row(page, /^second/).click(); await page.keyboard.press('Control+v');
  await expect.poll(() => onDisk('web/second/app.ts')).toBe(original);
  expect(await onDisk('web/target/app.ts')).toBeNull(); expect(await onDisk('web/src/app.ts')).toBe(original);
});
