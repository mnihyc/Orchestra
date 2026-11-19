/** Production UI with the native-shaped test API: drafts survive presentation changes and local-save failures. */
import { expect, test, type Page } from '@playwright/test';
import { scenario, signIn } from './support';
import { DOCS, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

const comments = (page: Page) => page.getByRole('region', { name: 'Comments on this document' });
async function prepare(page: Page) {
  await scenario(); await signIn(page); await openDocument(page, DOCS.plan);
  await showWorkbenchView(page, 'Comments');
  const panel = comments(page);
  await panel.getByRole('button', { name: 'Start a discussion', exact: true }).click();
  const id = await panel.evaluate((element) => element.closest('[data-view-instance]')?.getAttribute('data-view-instance'));
  if (id === null || id === undefined) throw new Error('The actual Comments view must own an identity');
  return id;
}
async function detach(page: Page, id: string) {
  const bar = page.locator('[data-workbench-bar="right"][data-active-container="document"]');
  const section = bar.locator('[data-section-view="comments"]').filter({ has: page.locator(`[data-view-instance="${id}"]`) });
  await expect(section).toBeVisible();
  const header = section.locator('[data-section-header]');
  if (await header.count() > 0) await header.getByRole('button', { name: /^(Comments actions|Actions for Comments)$/ }).click();
  else {
    // A lone section carries its exact view menu in the existing container header.
    await expect(bar.locator('[data-section-view]')).toHaveCount(1);
    await bar.locator('[data-container-title]').getByRole('button', { name: 'Document actions', exact: true }).click();
  }
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).hover();
  const destination = page.getByRole('menuitem', { name: 'Split right', exact: true });
  const rect = await destination.boundingBox(); if (rect === null) throw new Error('The existing split destination must be visible');
  await page.mouse.move(rect.x + rect.width / 2, rect.y + rect.height / 2, { steps: 20 }); await destination.click();
  await expect(page.locator(`#tab-${id}`)).toBeVisible();
}
async function close(page: Page, id: string) {
  await page.locator(`#tab-${id}`).click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
}
async function reopen(page: Page) {
  await page.getByRole('button', { name: 'Layout', exact: true }).first().click();
  await page.getByRole('menuitem', { name: 'Reopen closed tab', exact: true }).click();
}
for (const theme of ['light', 'dark'] as const) test(`unsent comments survive detach, close/reopen and refresh in ${theme}`, async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  const errors: string[] = [], writes: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => { if (request.method() !== 'GET' && /\/documents\/[^/]+\/(discussions|comments)/.test(request.url())) writes.push(request.url()); });
  const id = await prepare(page), field = comments(page).getByRole('textbox', { name: 'New discussion' });
  await field.fill('Unsent words stay with this document');
  await page.keyboard.press('Home'); await page.keyboard.press('ArrowRight'); await page.keyboard.press('Shift+ArrowRight'); await page.keyboard.press('Shift+ArrowRight');
  const selection = await field.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection]);
  await detach(page, id); await expect(field).toHaveValue('Unsent words stay with this document');
  await close(page, id); await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(comments(page)).toHaveCount(0); await reopen(page);
  await expect(field).toHaveValue('Unsent words stay with this document');
  await page.reload(); await expect(field).toHaveValue('Unsent words stay with this document');
  expect(await field.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection])).toEqual(selection);
  await expect(page).toHaveURL(new RegExp(`/doc/${DOCS.plan}$`)); expect(writes).toEqual([]);
  await page.screenshot({ path: info.outputPath(`comments-recovered-${theme}.png`), animations: 'disabled' });
  await comments(page).getByRole('button', { name: 'Send', exact: true }).click();
  await expect(comments(page).getByRole('region', { name: 'Discussion', exact: true })).toContainText('Unsent words stay with this document');
  await expect(comments(page).getByRole('region', { name: 'New discussion', exact: true })).toHaveCount(0);
  expect(writes).toHaveLength(1); expect(errors).toEqual([]);
});

test('a failed local write keeps the actual pane and draft reachable until Save succeeds', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); const id = await prepare(page); await detach(page, id);
  await page.evaluate(() => {
    const original = Object.getOwnPropertyDescriptor(Storage.prototype, 'setItem');
    if (original === undefined) throw new Error('Native browser storage setter is missing');
    const local = localStorage.setItem.bind(localStorage), session = sessionStorage.setItem.bind(sessionStorage);
    Storage.prototype.setItem = function (this: Storage, key: string, value: string) {
      if (key.includes('.unsaved.document-comments.')) throw new DOMException('Storage refused by the test', 'QuotaExceededError');
      (this === localStorage ? local : session)(key, value);
    };
    window.addEventListener('restore-comment-storage', () => { Object.defineProperty(Storage.prototype, 'setItem', original); }, { once: true });
  });
  const field = comments(page).getByRole('textbox', { name: 'New discussion' });
  await field.fill('Keep the only current copy');
  await expect(comments(page).getByRole('button', { name: 'Try again', exact: true })).toBeVisible();
  await close(page, id);
  const decision = page.getByRole('dialog', { name: 'Save changes before closing?' });
  await decision.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(field).toHaveValue('Keep the only current copy'); await close(page, id);
  await decision.getByRole('button', { name: 'Save all', exact: true }).click();
  await expect(decision).toBeVisible(); await expect(page.locator(`#tab-${id}`)).toBeAttached();
  await expect(decision.getByRole('alert')).toHaveText('Some changes could not be saved. Their views remain open.');
  await page.screenshot({ path: info.outputPath('comments-local-save-failed.png'), animations: 'disabled' });
  await page.evaluate(() => window.dispatchEvent(new Event('restore-comment-storage')));
  await decision.getByRole('button', { name: 'Save all', exact: true }).click(); await expect(decision).toHaveCount(0);
  await expect(comments(page)).toHaveCount(0); await reopen(page); await page.reload();
  await expect(field).toHaveValue('Keep the only current copy');
});
