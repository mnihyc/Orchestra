/** Independent windows of one real thread keep source, paging, tool bodies and drafts separate. */
import { expect, test, type Page } from '@playwright/test';
import { begin, MAIN, openThread } from './inspector-support';
import { A, B, COMMAND_ITEM, ITEM_A, ITEM_B, ROOT_ITEM, SHARED_ITEM, versions } from './prompt-version-support';

async function openBeside(page: Page) {
  await page.keyboard.press('Control+k');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('/ unique retained source');
  await finder.getByRole('option', { name: /^unique retained source\b/ }).click({ modifiers: ['Alt'] });
  const preview = page.getByRole('region', { name: 'Search result', exact: true });
  await expect(preview).toBeVisible();
  return preview;
}

test('two histories of the same item use one socket and never replace the route’s rows, tool expansion or draft', async ({ page, context }, info) => {
  await begin(); await page.setViewportSize({ width: 1440, height: 900 });
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await versions(page, true, { shared: true, searchView: A });
  const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message));
  await openThread(page);
  const main = page.locator('[data-screen="thread"]');
  const draft = main.getByRole('textbox', { name: 'Message to the agent', exact: true });
  await draft.fill('Keep the main conversation draft');
  await expect(main.getByText('Frozen source keeps its own text', { exact: true })).toBeVisible();
  const connections = fixture.connections();
  const preview = await openBeside(page);
  await expect(preview.getByText('Original source keeps its own text', { exact: true })).toBeVisible();
  await expect(main.getByText('Frozen source keeps its own text', { exact: true })).toBeVisible();
  await expect(draft).toHaveValue('Keep the main conversation draft');
  await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}$`));
  expect(fixture.connections()).toBe(connections);
  expect(fixture.subscriptions.map((one) => one.continuation).sort()).toEqual([null, A].sort());
  expect(new Set(fixture.subscriptions.map((one) => one.id)).size).toBe(2);
  expect(fixture.subscriptions.every((one) => one.feed === `thread:${MAIN}`)).toBe(true);

  const previewTool = preview.locator(`[data-item="${COMMAND_ITEM}"]`);
  await previewTool.locator('[data-tool-row] > button').click();
  await previewTool.getByRole('button', { name: /^Show all/ }).click();
  await expect(previewTool.getByText('Complete original output', { exact: true })).toBeVisible();
  await expect(main.locator(`[data-item="${COMMAND_ITEM}"] [data-tool-row] > button`)).toHaveAttribute('aria-expanded', 'false');
  const outputRead = fixture.calls.find((one) => one.path.endsWith('/output'));
  expect(outputRead?.query).toContain(`continuation_id=${A}`);

  await preview.getByRole('button', { name: 'Find in the conversation', exact: true }).click();
  const find = preview.getByRole('search', { name: 'Find in the conversation', exact: true });
  await find.getByRole('textbox').fill('retained source');
  await expect(find.getByRole('status')).toHaveText('1 of 1');
  expect(fixture.calls.filter((one) => one.path.endsWith('/search') && one.query.includes('scope=')).every((one) =>
    one.query.includes(`scope=${MAIN}`) && one.query.includes(`continuation_id=${A}`))).toBe(true);
  await find.getByRole('textbox').press('Escape');
  await preview.locator(`[data-item="${SHARED_ITEM}"]`).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy link', exact: true }).click();
  const link = new URL(await page.evaluate(() => navigator.clipboard.readText()));
  expect(link.searchParams.get('continuation')).toBe(A); expect(link.searchParams.get('item')).toBe(SHARED_ITEM);

  await preview.getByRole('button', { name: 'Next version', exact: true }).click();
  await expect(preview.getByRole('group', { name: 'Message versions', exact: true })).toContainText('2/2');
  await expect(main.getByText('Frozen source keeps its own text', { exact: true })).toBeVisible();
  await expect(draft).toHaveValue('Keep the main conversation draft');
  await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}$`));
  expect(fixture.calls.filter((one) => one.method === 'POST' && !one.path.endsWith('/read'))).toEqual([]);
  await page.mouse.move(0, 0); await page.keyboard.press('Escape');
  await page.screenshot({ path: info.outputPath('independent-thread-histories.png'), fullPage: true, animations: 'disabled' });
  await preview.getByRole('button', { name: 'Return to the conversation', exact: true }).click();
  await expect(preview).toHaveCount(0); await expect(draft).toHaveValue('Keep the main conversation draft');
  expect(errors).toEqual([]);
});

test('two windows of the same continuation keep independent anchors when the preview jumps to its tail', async ({ page }, info) => {
  await begin(); await page.setViewportSize({ width: 1440, height: 900 });
  const fixture = await versions(page, true, { windowed: true, searchView: B });
  await openThread(page, undefined, `/thread/${MAIN}?item=${ROOT_ITEM}`);
  const main = page.locator('[data-screen="thread"]');
  await expect(main.getByText('The main view stays at this anchor', { exact: true })).toBeVisible();
  const preview = await openBeside(page);
  await expect(preview.getByText('Frozen source keeps its own text', { exact: true })).toBeVisible();
  await expect(main.getByText('The main view stays at this anchor', { exact: true })).toBeVisible();
  await preview.getByRole('button', { name: 'Jump to latest', exact: true }).click();
  await expect(preview.getByText('Later history item 219', { exact: true })).toBeVisible();
  await expect(main.getByText('The main view stays at this anchor', { exact: true })).toBeVisible();
  await expect(main.getByText('Later history item 219', { exact: true })).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`item=${ROOT_ITEM}`));
  const around = fixture.calls.filter((one) => one.path.endsWith('/items'));
  expect(around.some((one) => one.query.includes(`around=${ROOT_ITEM}`))).toBe(true);
  expect(around.some((one) => one.query.includes(`around=${SHARED_ITEM}`) && one.query.includes(`continuation_id=${B}`))).toBe(true);
  await page.screenshot({ path: info.outputPath('independent-thread-windows.png'), fullPage: true, animations: 'disabled' });
});

test('version arrows retain a prompt outside the 200-item tail and restore keyboard focus', async ({ page }, info) => {
  await begin(); const fixture = await versions(page, true, { windowed: true });
  await openThread(page, undefined, `/thread/${MAIN}?item=${ITEM_B}`);
  const main = page.locator('[data-screen="thread"]');
  const strip = main.getByRole('group', { name: 'Message versions', exact: true });
  await expect(strip).toContainText('2/2');
  await strip.getByRole('button', { name: 'Previous version', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(strip).toContainText('1/2');
  await expect(main.getByText('Original release plan', { exact: true })).toBeVisible();
  await expect(strip.getByRole('button', { name: 'Next version', exact: true })).toBeFocused();
  expect(await main.locator(`[data-item="${ITEM_A}"]`).getAttribute('class')).not.toContain('ring-2');
  await expect(page).toHaveURL(new RegExp(`item=${ITEM_A}`));
  expect(fixture.calls.filter((one) => one.path.endsWith('/items')).some((one) =>
    one.query.includes(`around=${ITEM_A}`) && one.query.includes(`continuation_id=${A}`))).toBe(true);
  expect(fixture.calls.filter((one) => one.method === 'POST' && !one.path.endsWith('/read'))).toEqual([]);
  await page.keyboard.press('Escape'); await page.screenshot({ path: info.outputPath('long-history-version-focus.png'), fullPage: true, animations: 'disabled' });
});
