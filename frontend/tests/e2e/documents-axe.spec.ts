/** Every screen and state of documents asked of axe in both themes: the page, its bars and menus, the panels and dialogs, the list beside a place, and the page of a reader and of an earlier version.
 *
 * What a list that opened over the page is asked on its own (`within`), as the shared helper says: the page behind
 * it is held away by design.
 */
import { expect, test } from '@playwright/test';

import { ACME, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from './support.ts';
import { actAs, block, caretAtEndOf, cutVersion, DOCS, editorOf, lineOf, openDocument, waitUntilSaved } from './documents-support.ts';

test.beforeEach(async () => {
  await scenario();
});

test('the page of a document, with every kind of block, is accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await expect(editorOf(page).locator('.katex').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the bar over words, the block menu and the handle menu are accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await lineOf(page, 'Ship the redirect fix').click();
  await page.keyboard.press('Home');
  await page.keyboard.press('Control+Shift+ArrowRight');
  const bar = page.getByRole('toolbar', { name: 'Text formatting' });
  await expect(bar).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="toolbar"]');

  // The block menu, opened at a slash in a new line.
  await caretAtEndOf(page, 'A bulleted line');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/');
  await expect(page.getByRole('listbox', { name: 'Blocks' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="listbox"]');
  await page.keyboard.press('Escape');

  // The handle's menu.
  await lineOf(page, 'Canary on staging').hover();
  await page.getByRole('button', { name: 'Block menu' }).click();
  await expect(page.getByRole('menu')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.keyboard.press('Escape');
});

test('the comments panel with a discussion in it is accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await lineOf(page, 'Canary on staging').hover();
  await page.getByRole('button', { name: 'Block menu' }).click();
  await page.getByRole('menuitem', { name: 'Comment' }).click();
  const panel = page.getByRole('region', { name: 'Comments on this document' });
  await panel.getByRole('textbox', { name: 'New discussion' }).fill('Who runs the canary?');
  // The composer is open: the panel as it is while somebody writes.
  await expectAccessibleInBothThemes(page, '[aria-label="Comments on this document"]');
  await panel.getByRole('button', { name: 'Send' }).click();
  await expect(panel.getByRole('region', { name: 'Discussion' })).toContainText('Who runs the canary?');
  await expectAccessibleInBothThemes(page, '[aria-label="Comments on this document"]');
});

test('the history drawer, with an entry chosen and its changes shown, is accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' ONE');
  await waitUntilSaved(page);
  await cutVersion(DOCS.plan);
  await page.getByRole('button', { name: 'History', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'History of Rollout plan' });
  await expect(drawer.getByRole('list', { name: 'Entries of the history' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await drawer.locator('[data-version="1"]').click();
  await expect(drawer.getByRole('region', { name: 'What this entry changed' })).toBeVisible();
  await expect(drawer.getByRole('list', { name: 'Blocks that differ' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the export, move, private, delete and share dialogs are accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  const open = async (entry: string | RegExp) => {
    await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
    await page.getByRole('menuitem', { name: entry }).click();
  };
  await open('Export…');
  const exporting = page.getByRole('dialog', { name: 'Export Rollout plan' });
  await expect(exporting).toBeVisible();
  // The fallback print dialog remains accessible on a device without workers.
  await expect(exporting.getByText('Markdown cannot say:')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.evaluate(() => { Object.defineProperty(window, 'Worker', { value: undefined, configurable: true }); });
  await exporting.getByRole('radio', { name: /^PDF/ }).check();
  await expect(exporting.getByRole('button', { name: 'Print' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await open('Move to…');
  const move = page.getByRole('dialog', { name: 'Move “Rollout plan”' });
  await move.getByRole('option', { name: /Upgrade deps/ }).click();
  await expect(move.getByText('Ownership stays the same.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await open(/Make private/);
  await expect(page.getByRole('alertdialog').or(page.getByRole('dialog', { name: 'Make this document private?' }))).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"], [role="dialog"]');
  await page.keyboard.press('Escape');

  await open('Delete');
  await expect(page.getByRole('alertdialog').or(page.getByRole('dialog', { name: 'Delete “Rollout plan”?' }))).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"], [role="dialog"]');
  await page.keyboard.press('Escape');

  // Share is the permission page every thing has: its dialog is named for the document.
  await page.getByRole('button', { name: 'Share' }).click();
  await expect(page.getByRole('dialog', { name: 'Access to “Rollout plan”' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the list of documents beside a topic, and its recently deleted, are accessible in both themes', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByRole('link', { name: 'Rollout plan' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: /^Recently deleted \(\d+\+?\)$/ }).click();
  // The joined retained-content world supplies a deleted document beside this topic.
  const deleted = page.getByRole('listitem').filter({ has: page.getByText('Investigation notes', { exact: true }) });
  await expect(deleted).toBeVisible();
  await expect(deleted.getByRole('button', { name: 'Restore Investigation notes', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  // A list with nothing in it.
  await page.goto(`/topic/${ACME.upgradeDeps}?tab=documents`);
  await expect(page.getByText(/Nothing written here yet/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the change that came second, kept beside the first with its three actions, is accessible in both themes', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' MINE');
  await actAs('grace', DOCS.plan, [{ op: 'update', id: block(2), base_seq: 1, content: [{ type: 'text', text: 'Ship it the other way' }] }]);
  await expect(page.getByRole('group', { name: 'Your text, kept beside another change' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a reader\'s page and an earlier version, shown as it stood, are accessible in both themes', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await openDocument(page, DOCS.plan);
  await expect(page.getByText('You can read this document. To write in it, ask Ada Lovelace, who owns it.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  // An earlier version, shown as it stood.
  await cutVersion(DOCS.plan);
  await actAs('ada', DOCS.plan, [{ op: 'update', id: block(2), base_seq: 1, content: [{ type: 'text', text: 'Changed once.' }] }]);
  await cutVersion(DOCS.plan);
  await page.goto(`/doc/${DOCS.plan}?version=1`);
  await expect(page.getByText('You are reading version 1, as it stood then.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
