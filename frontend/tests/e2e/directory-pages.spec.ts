/** Large native-shaped directory pages stay reachable without eager walking or losing edits. */
import { expect, test } from '@playwright/test';
import { begin, fileTree, filesPanel, MAIN_ENVIRONMENT, onDisk, openThread, row, showView, workspace } from './inspector-support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';

for (const [width, theme] of [[1440, 'light'], [1440, 'dark'], [400, 'light'], [400, 'dark']] as const) {
  test(`5007 siblings have explicit adjacent pages at ${width}/${theme}`, async ({ page }, info) => {
    await begin('many'); await workspace('/fill', { directory: 'many', files: 5007 });
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const reads: string[] = [], errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (request.method() === 'GET' && url.pathname === `/api/v1/environments/${MAIN_ENVIRONMENT}/files`) reads.push(url.search);
    });
    await openThread(page); await showView(page, /^Files/);
    const panel = filesPanel(page);
    await expect(row(page, /^file-00000/)).toBeVisible();
    await expect(panel.getByText('Page 1', { exact: true })).toBeVisible();
    expect(await fileTree(page).getByRole('treeitem').count()).toBeLessThan(100);
    expect(reads.filter((read) => new URLSearchParams(read).has('cursor'))).toEqual([]);
    await row(page, /^file-00000/).focus(); await page.keyboard.press('End');
    await expect(row(page, /^file-04999/)).toBeFocused();
    await panel.getByRole('button', { name: 'Next directory page', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(panel.getByText('Page 2', { exact: true })).toBeVisible();
    await expect(row(page, /^file-05000/)).toBeVisible();
    await expect(row(page, /^file-05006/)).toBeVisible();
    expect(await fileTree(page).getByRole('treeitem').count()).toBe(7);
    await expect(panel.getByRole('button', { name: 'Next directory page', exact: true })).toBeDisabled();
    expect(reads.filter((read) => new URLSearchParams(read).has('cursor'))).toHaveLength(1);
    await page.mouse.move(0, 0); await page.evaluate(() => document.fonts.ready);
    const screenshot = info.outputPath(`directory-page-${width}-${theme}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('directory-page', { path: screenshot, contentType: 'image/png' });
    const before = reads.length;
    await panel.getByRole('button', { name: 'Previous directory page', exact: true }).click();
    await expect(row(page, /^file-00000/)).toBeVisible();
    expect(reads).toHaveLength(before);
    expect(errors).toEqual([]);
  });
}

test('a dirty late-page file follows a wide invalidation, and search opens its parent directly', async ({ page }, info) => {
  await begin('many'); await workspace('/fill', { directory: 'many', files: 5007 });
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThread(page); await showView(page, /^Files/);
  const panel = filesPanel(page);
  await panel.getByRole('button', { name: 'Next directory page', exact: true }).click();
  await row(page, /^file-05006/).click();
  const code = page.getByRole('textbox', { name: 'Text of file-05006.txt', exact: true });
  await expect(code).toContainText('file 5006');
  await code.click(); await page.keyboard.press('Control+Home'); await page.keyboard.type('unsaved\n');
  const reads: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() === 'GET' && url.pathname === `/api/v1/environments/${MAIN_ENVIRONMENT}/files`) reads.push(url.search);
  });
  await workspace('/put', { files: { 'many/file-05006.txt': 'changed on disk\n' } });
  await expect(page.getByText('This file changed while you were editing', { exact: true })).toBeVisible();
  await expect(code).toContainText('unsaved');
  await expect(panel.getByText('Page 2', { exact: true })).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Next directory page', exact: true })).toBeDisabled();
  expect(reads).toEqual([]);
  await panel.getByRole('button', { name: 'Search the files', exact: true }).click();
  await panel.getByRole('textbox', { name: 'Search the files', exact: true }).fill('file-05006');
  await page.getByTestId('search-results').getByRole('button', { name: 'Open parent folder', exact: true }).click();
  await expect(panel.getByRole('heading', { name: 'many/ in Workspace', exact: true })).toBeVisible();
  await expect(code).toContainText('unsaved');
  await panel.getByRole('button', { name: 'Restart directory listing', exact: true }).click();
  await expect(panel.getByText('Page 1', { exact: true })).toBeVisible();
  const idle = reads.length;
  await page.waitForTimeout(60_000);
  expect(reads).toHaveLength(idle);
  const screenshot = info.outputPath('dirty-late-page.png');
  await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
  await info.attach('dirty-late-page', { path: screenshot, contentType: 'image/png' });
});

test('nested page controls and a removed row preserve actual keyboard focus without stealing it', async ({ page }) => {
  await begin(); await workspace('/fill', { directory: 'web/wide', files: 5007 });
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThread(page); await showView(page, /^Files/);
  await row(page, /^wide/).focus(); await page.keyboard.press('ArrowRight');
  await expect(row(page, /^file-00000/)).toBeVisible(); await page.keyboard.press('ArrowRight');
  await expect(row(page, /^file-00000/)).toBeFocused();
  await page.keyboard.type('file-04999'); await expect(row(page, /^file-04999/)).toBeFocused();
  await page.keyboard.press('ArrowDown');
  const pager = fileTree(page).getByRole('treeitem', { name: 'Pages in web/wide', exact: true });
  await expect(pager).toBeFocused(); await page.keyboard.press('Enter');
  await expect(row(page, /^file-05000/)).toBeFocused();
  await page.keyboard.type('file-05006'); await expect(row(page, /^file-05006/)).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(pager).toBeFocused();
  await page.keyboard.press('Enter'); await expect(row(page, /^file-00000/)).toBeFocused();
  await workspace('/delete', { path: 'web/wide/file-00000.txt' });
  await expect(row(page, /^wide/)).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(row(page, /^file-00001/)).toBeFocused();
  await filesPanel(page).getByRole('button', { name: 'Search the files', exact: true }).click();
  const input = filesPanel(page).getByRole('textbox', { name: 'Search the files', exact: true });
  await expect(input).toBeFocused();
  await workspace('/delete', { path: 'web/wide/file-00001.txt' });
  await expect(input).toBeFocused();
});

test('a drag keeps its source workspace and a foreign same-named file is never moved locally', async ({ page }) => {
  await begin(); const other = RUNS_ON.docsCheckout;
  await workspace('/put', { files: { 'web/report.txt': 'local report', 'web/archive/.keep': '' } });
  await workspace('/put', { environment: other, files: { 'web/report.txt': 'foreign report' } });
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThread(page); await showView(page, /^Files/);
  const moves: string[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/files/rename')) moves.push(request.url()); });
  const foreign = await page.evaluateHandle((environmentId) => {
    const transfer = new DataTransfer();
    transfer.setData('application/x-orch-paths', JSON.stringify({ environmentId, paths: ['web/report.txt'] }));
    return transfer;
  }, other);
  await row(page, /^archive/).dispatchEvent('drop', { dataTransfer: foreign });
  await expect(page.getByText('These items belong to another workspace.', { exact: true })).toBeVisible();
  expect(moves).toEqual([]); expect(await onDisk('web/report.txt')).toBe('local report');
  expect((await workspace('/content', { environment: other, path: 'web/report.txt' }) as { text: string }).text).toBe('foreign report');
  const local = await page.evaluateHandle(() => new DataTransfer());
  await row(page, /^report\.txt/).dispatchEvent('dragstart', { dataTransfer: local });
  expect(await local.evaluate((transfer) => JSON.parse(transfer.getData('application/x-orch-paths')) as unknown))
    .toEqual({ environmentId: MAIN_ENVIRONMENT, paths: ['web/report.txt'] });
  await row(page, /^archive/).dispatchEvent('drop', { dataTransfer: local });
  await expect.poll(() => onDisk('web/archive/report.txt')).toBe('local report');
  expect(await onDisk('web/report.txt')).toBeNull(); expect(moves).toHaveLength(1);
  expect((await workspace('/content', { environment: other, path: 'web/report.txt' }) as { text: string }).text).toBe('foreign report');
  await local.dispose(); await foreign.dispose();
});

test('an open dirty file explains a failed watch and retries without losing its draft', async ({ page }, info) => {
  await begin(); await page.setViewportSize({ width: 1440, height: 900 });
  await openThread(page); await showView(page, /^Files/);
  await row(page, /^src/).click(); await row(page, /^app\.ts/).click();
  const file = page.locator('[data-file-tab="web/src/app.ts"]');
  const code = file.getByRole('textbox', { name: 'Text of app.ts', exact: true });
  await code.click(); await page.keyboard.press('Control+Home'); await page.keyboard.type('// local draft\n');
  await workspace('/directory-event', { path: 'web/src', kind: 'failure' });
  await expect(file.getByRole('alert')).toContainText('Live file updates are paused.');
  await expect(page.getByTestId('files-panel').getByText('Snapshot', { exact: true })).toBeVisible();
  await expect(code).toContainText('// local draft');
  const path = info.outputPath('dirty-file-watch-failed.png');
  await page.screenshot({ path, fullPage: true, animations: 'disabled' });
  await info.attach('dirty-file-watch-failed', { path, contentType: 'image/png' });
  await file.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(file.getByText('Live file updates are paused.', { exact: true })).toHaveCount(0);
  await expect(page.getByTestId('files-panel').getByText('Live', { exact: true })).toBeVisible();
  await expect(code).toContainText('// local draft');
  await expect(page.getByTestId('code-status')).toContainText('Unsaved');
});

for (const [width, theme] of [[1440, 'light'], [1440, 'dark'], [400, 'light'], [400, 'dark']] as const) {
  test(`language is a compact searchable field at ${width}/${theme}`, async ({ page }, info) => {
    await begin(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await openThread(page); await showView(page, /^Files/);
    await row(page, /^src/).click(); await row(page, /^app\.ts/).click();
    const file = page.locator('[data-file-tab="web/src/app.ts"]');
    const field = file.getByRole('combobox', { name: 'Language', exact: true });
    await expect(field).toHaveValue('TypeScript');
    const sizes = await field.evaluate((element) => {
      const token = getComputedStyle(element).getPropertyValue('--control-sm').trim();
      const font = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
      return { actual: element.getBoundingClientRect().height, expected: Number.parseFloat(token) * (token.endsWith('rem') ? font : 1) };
    });
    expect(Math.abs(sizes.actual - sizes.expected)).toBeLessThanOrEqual(1);
    const writes: string[] = [];
    page.on('request', (request) => { if (request.method() === 'PUT' && new URL(request.url()).pathname.endsWith('/files/content')) writes.push(request.url()); });
    await field.fill('py');
    await expect(page.getByRole('option', { name: 'Python', exact: true })).toBeVisible();
    await expect(page.getByRole('option')).toHaveCount(1);
    const path = info.outputPath(`language-search-${width}-${theme}.png`);
    await page.screenshot({ path, fullPage: true, animations: 'disabled' });
    await info.attach('language-search', { path, contentType: 'image/png' });
    await field.press('Enter'); await expect(field).toHaveValue('Python');
    await field.fill('no-such-language');
    await expect(page.getByText('No matching language.', { exact: true })).toBeVisible();
    await field.press('Escape'); await expect(field).toHaveValue('Python');
    expect(writes).toEqual([]);
  });
}
