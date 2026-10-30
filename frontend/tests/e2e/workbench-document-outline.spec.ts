/** Real workbench controls and document editors; setup uses the existing authorized document fixtures. */
import { expect, test, type Locator, type Page } from '@playwright/test';
import { scenario, signIn } from './support';
import { actAs, cutVersion, DOCS, held, makeLong, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

const DESTINATION = 'Outline target heading';
const panes = (page: Page, id: string) => page.locator(`[data-view-instance]:has(article[data-document="${id}"])`);
const pane = (page: Page, owner: string) => page.locator(`[data-view-instance="${owner}"]`);
const tab = (page: Page, owner: string) => page.locator(`#tab-${owner}`);
async function identity(view: Locator): Promise<string> {
  const id = await view.getAttribute('data-view-instance');
  if (id === null) throw new Error('The actual document view must retain its owner ID.');
  return id;
}
async function prepare(page: Page) {
  await scenario(); await signIn(page);
  // Plain paragraphs avoid media-dependent geometry; the assertions below require actual scroll room.
  const documentId = await makeLong(40, 'Outline target proof');
  const first = (await held(documentId)).blocks[0];
  if (first === undefined) throw new Error('The ordinary document fixture must contain a first block.');
  await actAs('ada', documentId, [{ op: 'update', id: first.id, base_seq: first.seq_updated,
    type: 'heading', props: { level: 1 }, content: [{ type: 'text', text: DESTINATION }] }]);
  return { documentId, blockId: first.id };
}
async function showOutline(page: Page) {
  // The persisted frame may have begun on Home; reveal both native document sections so Outline has its own header menu.
  await showWorkbenchView(page, 'Comments');
  await showWorkbenchView(page, 'Outline');
  const outline = page.locator('[data-section-view="outline"]');
  await expect(outline.getByRole('button', { name: new RegExp(DESTINATION) })).toBeVisible();
  return outline;
}
async function closeTab(page: Page, owner: string) {
  await tab(page, owner).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
  await expect(tab(page, owner)).toHaveCount(0);
}

for (const theme of ['light', 'dark'] as const) test(`Follow Outline reveals only the second duplicate document in ${theme}`, async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: theme }); await page.setViewportSize({ width: 1440, height: 900 });
  const { documentId, blockId } = await prepare(page); await openDocument(page, documentId);
  const one = await identity(panes(page, documentId));
  await tab(page, one).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Open beside', exact: true }).click();
  await expect(panes(page, documentId)).toHaveCount(2);
  const ids = await panes(page, documentId).evaluateAll((elements) => elements.map((element) => element.getAttribute('data-view-instance')));
  const two = ids.find((id) => id !== null && id !== one);
  if (two === undefined || two === null) throw new Error('Open beside must create a separate document owner.');
  const outline = await showOutline(page);
  await tab(page, two).click(); await expect(tab(page, two)).toBeFocused();
  await page.evaluate(() => document.fonts.ready);
  const firstScroll = pane(page, one).locator('[data-document-scroll]'), secondScroll = pane(page, two).locator('[data-document-scroll]');
  const beforeFirst = await firstScroll.evaluate((element) => { element.scrollTop = (element.scrollHeight - element.clientHeight) / 2; return element.scrollTop; });
  const beforeSecond = await secondScroll.evaluate((element) => { element.scrollTop = element.scrollHeight; return element.scrollTop; });
  expect(beforeFirst).toBeGreaterThan(0); expect(beforeSecond).toBeGreaterThan(0);
  await expect(pane(page, two).locator(`[data-id="${blockId}"]`)).not.toBeInViewport();
  const writes: string[] = [];
  page.on('request', (request) => { if (request.method() !== 'GET' && /\/documents\/[^/]+\/ops$/.test(new URL(request.url()).pathname)) writes.push(request.url()); });
  await outline.getByRole('button', { name: new RegExp(DESTINATION) }).click();
  await expect(pane(page, two).locator(`[data-id="${blockId}"]`)).toHaveClass(/is-highlighted/);
  await expect(pane(page, two).locator(`[data-id="${blockId}"]`)).toBeInViewport();
  await expect(pane(page, one).locator(`[data-id="${blockId}"]`)).not.toHaveClass(/is-highlighted/);
  expect(await firstScroll.evaluate((element) => element.scrollTop)).toBe(beforeFirst);
  await expect.poll(() => secondScroll.evaluate((element) => element.scrollTop)).toBeLessThan(beforeSecond);
  expect(writes).toEqual([]);
  await page.screenshot({ path: info.outputPath(`outline-second-owner-${theme}.png`), animations: 'disabled' });
});

for (const theme of ['light', 'dark'] as const) test(`fixed Outline restores its own versioned document and opens its target after closure in ${theme}`, async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: theme }); await page.setViewportSize({ width: 1440, height: 900 });
  const { documentId, blockId } = await prepare(page);
  await cutVersion(documentId); const version = (await held(documentId)).versions;
  expect(version).toBeGreaterThan(0);
  const writes: string[] = [];
  page.on('request', (request) => { if (request.method() !== 'GET' && /\/documents\/[^/]+\/ops$/.test(new URL(request.url()).pathname)) writes.push(request.url()); });
  await page.goto(`/doc/${documentId}?version=${version}#outline-proof`);
  await expect(panes(page, documentId).getByRole('textbox', { name: 'Document text', exact: true })).toBeVisible();
  const original = await identity(panes(page, documentId));
  const outline = await showOutline(page);
  await outline.getByRole('button', { name: 'Outline', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Fixed target', exact: true }).click();
  // A copied document address is an ordinary existing navigation entrance; pagehide persists the fixed scope.
  await openDocument(page, DOCS.notes); const other = await identity(panes(page, DOCS.notes));
  await expect(tab(page, original)).toBeVisible();
  const fixedOutline = page.locator('[data-section-view="outline"]');
  await fixedOutline.getByRole('button', { name: new RegExp(DESTINATION) }).click();
  await expect(tab(page, original)).toHaveAttribute('aria-selected', 'true');
  await expect(pane(page, original).locator(`[data-id="${blockId}"]`)).toHaveClass(/is-highlighted/);
  await expect.poll(() => {
    const url = new URL(page.url()); return [url.pathname, url.searchParams.get('version'), url.searchParams.get('block'), url.hash];
  }).toEqual([`/doc/${documentId}`, String(version), blockId, '#outline-proof']);
  expect(await identity(panes(page, documentId))).toBe(original); await expect(tab(page, other)).toBeVisible();
  await closeTab(page, original); await tab(page, other).click();
  await fixedOutline.getByRole('button', { name: new RegExp(DESTINATION) }).click();
  await expect(panes(page, documentId)).toHaveCount(1);
  const opened = await identity(panes(page, documentId)); expect(opened).not.toBe(original);
  await expect(pane(page, opened).locator(`[data-id="${blockId}"]`)).toHaveClass(/is-highlighted/);
  await expect(tab(page, other)).toBeVisible();
  // The resource jump creates a normal current view; the closed snapshot remains independently recoverable.
  await page.getByRole('button', { name: 'Layout', exact: true }).first().click();
  await page.getByRole('menuitem', { name: 'Reopen closed tab', exact: true }).click();
  await expect(tab(page, original)).toHaveAttribute('aria-selected', 'true'); await expect(tab(page, opened)).toBeVisible();
  await expect.poll(() => new URL(page.url()).searchParams.get('version')).toBe(String(version));
  await expect(pane(page, original).locator(`[data-id="${blockId}"]`)).toBeInViewport();
  expect(writes).toEqual([]);
  await page.screenshot({ path: info.outputPath(`outline-fixed-restored-${theme}.png`), animations: 'disabled' });
});
