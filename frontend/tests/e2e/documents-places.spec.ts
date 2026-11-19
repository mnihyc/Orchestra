/** Documents beside others: a discussion begun, answered and resolved, a version put back, what changed since the reader looked, a move, an export, a delete and its undoing, private and open again, and a reader who may not write.
 *
 * Against the fake API and its fake socket. What a test asserts about the server is what the fake server holds.
 */
import { expect, test } from '@playwright/test';
import { showWorkbenchView } from './workbench-support.ts';
import { readFile } from 'node:fs/promises';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';
import { installImportFixture } from './import-support.ts';
import { actAs, block, caretAtEndOf, cutVersion, DOCS, editorOf, held, lineOf, openDocument, saveState, waitUntilSaved, wordsOf } from './documents-support.ts';

test.beforeEach(async () => {
  await scenario();
});

test('a document named as its place README is shown there and can be cleared from the document menu', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  const actions = page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / });
  await actions.click();
  await page.getByRole('menuitem', { name: "Use as this place's README" }).click();
  await page.goto(`/topic/${ACME.fixLogin}`);
  const readme = page.locator(`[data-readme="${DOCS.plan}"]`);
  await expect(readme).toBeVisible();
  await expect(readme).toContainText('Ship the redirect fix');
  await openDocument(page, DOCS.plan);
  await actions.click();
  await page.getByRole('menuitem', { name: 'Stop using as README' }).click();
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(readme).toHaveCount(0);
});

test('a discussion is begun on some words, answered and resolved, and is then listed under Resolved', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  // The first word is selected; the bar over it offers Comment, and Comment opens the panel with the words quoted.
  await lineOf(page, 'Ship the redirect fix').click();
  await page.keyboard.press('Home');
  await page.keyboard.press('Control+Shift+ArrowRight');
  await page.getByRole('toolbar', { name: 'Text formatting' }).getByRole('button', { name: 'Comment' }).click();
  const panel = page.getByRole('region', { name: 'Comments on this document' });
  const compose = panel.getByRole('region', { name: 'New discussion' });
  await expect(compose.locator('q')).toHaveText('Ship');
  await compose.getByRole('textbox', { name: 'New discussion' }).fill('Why not behind the flag?');
  await compose.getByRole('button', { name: 'Send' }).click();

  const card = panel.getByRole('region', { name: 'Discussion' });
  await expect(card).toContainText('Why not behind the flag?');
  await expect(card.locator('q')).toHaveText('Ship');
  await expect(panel.getByRole('region', { name: 'Discussion' })).toHaveCount(1);
  // The words are marked in the page.
  await expect(editorOf(page).locator('.oc-commented')).toHaveText('Ship');

  await card.getByRole('textbox', { name: 'Answer' }).fill('It is behind the flag.');
  await card.getByRole('button', { name: 'Send' }).click();
  await expect(card).toContainText('It is behind the flag.');

  await card.getByRole('button', { name: 'Resolve' }).click();
  await expect(panel.getByText('No open discussions')).toBeVisible();
  await expect(editorOf(page).locator('.oc-commented')).toHaveCount(0);
  await page.getByRole('combobox', { name: 'Which discussions' }).click();
  await page.getByRole('option', { name: 'Resolved' }).click();
  await expect(panel.getByRole('region', { name: 'Discussion' })).toContainText('Resolved by Ada Lovelace');
  await panel.getByRole('button', { name: 'Open again' }).click();
  await expect(panel.getByText('No resolved discussions')).toBeVisible();

  // A reload finds the discussion where it was left.
  await page.reload();
  await showWorkbenchView(page, 'Comments');
  // The panel keeps its selected filter; reopening a discussion does not change that preference.
  await expect(page.getByRole('combobox', { name: 'Which discussions' })).toContainText('Resolved');
  await expect(panel.getByText('No resolved discussions')).toBeVisible();
  await page.getByRole('combobox', { name: 'Which discussions' }).click();
  await page.getByRole('option', { name: 'Open', exact: true }).click();
  await expect(panel.getByRole('region', { name: 'Discussion' })).toHaveCount(1);
});

test('a block comment keeps its target when the narrow workbench opens Comments', async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 900 });
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await lineOf(page, 'Canary on staging').hover();
  await page.getByRole('button', { name: 'Block menu' }).click();
  await page.getByRole('menuitem', { name: 'Comment' }).click();
  const panel = page.getByRole('region', { name: 'Comments on this document' });
  await panel.getByRole('textbox', { name: 'New discussion' }).fill('Keep this block');
  await panel.getByRole('button', { name: 'Send' }).click();
  const discussion = panel.getByRole('region', { name: 'Discussion', exact: true });
  await expect(discussion).toContainText('Canary on staging');
  await expect(discussion).toContainText('Keep this block');
});

test('a person choice card remains inside the viewport and outside its list after resize', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await lineOf(page, 'Ship the redirect fix').click();
  await page.keyboard.press('End');
  await page.keyboard.type(' @');
  const list = page.getByRole('listbox', { name: 'People' });
  await expect(list.getByRole('option').first()).toBeVisible();
  const card = page.locator('[data-inspection-card]:visible').first();
  await expect(card).toBeVisible();
  await page.setViewportSize({ width: 400, height: 900 });
  await expect.poll(async () => {
    const box = await card.boundingBox(), choices = await list.boundingBox();
    if (box === null || choices === null) return false;
    const inside = box.x >= 0 && box.y >= 0 && box.x + box.width <= 400 && box.y + box.height <= 900;
    const overlaps = box.x < choices.x + choices.width && box.x + box.width > choices.x && box.y < choices.y + choices.height && box.y + box.height > choices.y;
    return inside && !overlaps;
  }, { timeout: 30_000 }).toBe(true);
});

test('a block is commented on from its menu, and a reader who may not write sees the discussion and answers in it', async ({ page, browser }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await lineOf(page, 'Canary on staging').hover();
  await page.getByRole('button', { name: 'Block menu' }).click();
  await page.getByRole('menuitem', { name: 'Comment' }).click();
  const panel = page.getByRole('region', { name: 'Comments on this document' });
  await panel.getByRole('textbox', { name: 'New discussion' }).fill('Who runs the canary?');
  await panel.getByRole('button', { name: 'Send' }).click();
  // A discussion on a whole block names the block by its words, and marks no words of it.
  await expect(panel.getByRole('region', { name: 'Discussion' })).toContainText('Canary on staging');
  await expect(editorOf(page).locator('.oc-commented')).toHaveCount(0);

  // Somebody who may read and not write sees the discussion, and may answer in it: commenting is not writing.
  const context = await browser.newContext();
  const vera = await context.newPage();
  await signIn(vera, PEOPLE.vera);
  await openDocument(vera, DOCS.plan);
  await showWorkbenchView(vera, 'Comments');
  const seen = vera.getByRole('region', { name: 'Comments on this document' }).getByRole('region', { name: 'Discussion' });
  await expect(seen).toContainText('Who runs the canary?');
  await seen.getByRole('textbox', { name: 'Answer' }).fill('Dora does.');
  await seen.getByRole('button', { name: 'Send' }).click();
  await expect(seen).toContainText('Dora does.');
  // It arrives on the first page without a reload.
  await expect(panel.getByRole('region', { name: 'Discussion' })).toContainText('Dora does.');
  await context.close();
});

test('a version is shown as it stood and put back, and the present is kept as a version first', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  const words = 'Ship the redirect fix behind the flag, then:';
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' ONE');
  await waitUntilSaved(page);
  await cutVersion(DOCS.plan);
  await page.keyboard.type(' TWO');
  await waitUntilSaved(page);
  await cutVersion(DOCS.plan);
  expect((await held(DOCS.plan)).versions).toBe(2);

  await page.getByRole('button', { name: 'History', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'History of Rollout plan' });
  await expect(drawer.getByRole('list', { name: 'Entries of the history' })).toBeVisible();
  await drawer.locator('[data-version="1"]').click();
  const detail = drawer.getByRole('region', { name: 'What this entry changed' });
  await expect(detail).toBeVisible();

  // The version as it stood: read, and not written.
  await detail.getByRole('button', { name: 'Show the document then' }).click();
  await expect(page.getByText('You are reading version 1, as it stood then.')).toBeVisible();
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText(`${words} ONE`);
  await expect(lineOf(page, 'Ship the redirect fix')).not.toContainText('TWO');
  await expect(editorOf(page)).toHaveAttribute('contenteditable', 'false');
  await page.getByRole('button', { name: 'Show the present' }).click();
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText('ONE TWO');

  // Put back: the page is what the version said, and the present was kept first.
  await page.getByRole('button', { name: 'History', exact: true }).click();
  await drawer.locator('[data-version="1"]').click();
  await detail.getByRole('button', { name: 'Put this version back' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Put this version back?' }).or(page.getByRole('dialog', { name: 'Put this version back?' }));
  await confirm.getByRole('button', { name: 'Put this version back' }).click();
  await expect(confirm).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(lineOf(page, 'Ship the redirect fix')).not.toContainText('TWO');
  const now = await held(DOCS.plan);
  expect(wordsOf(now.blocks.find((one) => one.id === block(2))!)).toBe(`${words} ONE`);
  expect(now.versions).toBeGreaterThan(2);
});

test('what others changed since the reader last looked is told under the title, and marked in the page until it is dismissed', async ({ page }) => {
  await signIn(page);
  const seen = page.waitForResponse((response) => response.url().includes('/seen') && response.request().method() === 'POST');
  await openDocument(page, DOCS.plan);
  await seen;
  await page.goto('/inbox');
  await actAs('dora', DOCS.plan, [{ op: 'update', id: block(2), base_seq: 1, content: [{ type: 'text', text: 'Ship it on Tuesday.' }] }]);
  await openDocument(page, DOCS.plan);
  const line = page.getByRole('status').filter({ has: page.getByRole('button', { name: 'Review', exact: true }) });
  await expect(line).toBeVisible();
  await expect(line).toContainText('Changed since you were here');
  await expect(line).toContainText('Dora Developer');
  await expect(line).toContainText('1 block');
  // The reader's own changes are not news to them.
  await expect(editorOf(page).locator('.is-changed')).toHaveCount(0);
  await line.getByRole('button', { name: 'Review', exact: true }).click();
  await expect(editorOf(page).locator('.is-changed')).toHaveCount(1);
  await expect(editorOf(page).locator('.is-changed')).toContainText('Ship it on Tuesday.');
  await page.getByRole('button', { name: 'Hide the marks' }).click();
  await expect(editorOf(page).locator('.is-changed')).toHaveCount(0);
  await line.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await expect(line).toHaveCount(0);
});

test('a document is moved beside another place, and each list shows it where it now is', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Move to…', exact: true });
  await dialog.getByRole('combobox', { name: 'Destination', exact: true }).fill('Upgrade');
  await page.getByRole('option', { name: /Upgrade deps/ }).click();
  await expect(dialog.getByText('Move keeps documents in their original organization.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(page.getByText('Documents updated', { exact: true })).toBeVisible();
  await expect(page.locator('[data-access-state]').filter({ hasText: 'Uses Upgrade deps permissions' })).toBeVisible();

  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByTestId('main').getByRole('link', { name: 'Notes from the canary', exact: true })).toBeVisible();
  await expect(page.getByTestId('main').getByRole('link', { name: 'Rollout plan', exact: true })).toHaveCount(0);
  await page.goto(`/topic/${ACME.upgradeDeps}?tab=documents`);
  await expect(page.getByTestId('main').getByRole('link', { name: 'Rollout plan', exact: true })).toBeVisible();
});

test('Documents has one page tab on a topic, a side view on a project, and follows a thread to its topic', async ({ page }) => {
  await signIn(page);
  const side = page.getByRole('region', { name: 'Right bar' });
  const view = side.getByTestId('documents-view');

  // UA-30: a topic keeps the same list in its page tab, without a second side view or chooser entry.
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('tab', { name: 'Documents', exact: true }).click();
  const topicList = page.getByRole('tabpanel', { name: 'Documents', exact: true }).getByRole('region', { name: 'Documents', exact: true });
  await expect(topicList.getByRole('link', { name: 'Rollout plan' })).toBeVisible();
  await expect(topicList.getByRole('link', { name: 'Notes from the canary' })).toBeVisible();
  await expect(topicList.getByRole('button', { name: 'New document' })).toBeVisible();
  await expect(view).toHaveCount(0);
  await page.getByRole('button', { name: 'Add a view', exact: true }).first().click();
  const chooser = page.getByRole('dialog', { name: 'Views', exact: true });
  await expect(chooser).toBeVisible();
  await expect(chooser.locator('[data-value="commands:view.open.documents"]')).toHaveCount(0);
  await page.keyboard.press('Escape');

  // A project lists its own; the switch brings in those beside the places under it, each with where it is written.
  await page.goto(`/project/${ACME.web}`);
  await showWorkbenchView(page, 'Documents');
  await expect(view.getByRole('link', { name: 'Web handbook' })).toBeVisible();
  await expect(view.getByRole('link', { name: 'Rollout plan' })).toHaveCount(0);
  await view.getByRole('checkbox', { name: 'Include child places' }).click();
  await expect(view.getByRole('link', { name: 'Rollout plan' })).toBeVisible();
  await expect(view.getByText('Fix login ·').first()).toBeVisible();

  // A thread has no documents of its own: its topic's are listed, and the view follows the reader to another thread.
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Documents');
  await expect(view.getByRole('link', { name: 'Rollout plan' })).toBeVisible();
  await page.goto(`/thread/${ACME.depsThread}`);
  await showWorkbenchView(page, 'Documents');
  await expect(view.getByRole('link', { name: 'Rollout plan' })).toHaveCount(0);
});

test('a document is exported as Markdown: what Markdown cannot say is told before the file is made, and the server’s own list once it is written', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Export Rollout plan' });
  await expect(dialog.getByRole('radio', { name: 'Markdown' })).toBeChecked();
  // Before anything is saved the dialog says what Markdown cannot say of this document, from its blocks; a format that keeps it all says nothing.
  await expect(dialog.getByText('Markdown cannot say:')).toBeVisible();
  await expect(dialog.getByRole('listitem').filter({ hasText: /callout/ })).toHaveText(/1 callout becomes a quote/);
  await dialog.getByRole('radio', { name: 'HTML' }).check();
  await expect(dialog.getByText('Markdown cannot say:')).toHaveCount(0);
  await dialog.getByRole('radio', { name: 'Markdown' }).check();
  await expect(dialog.getByText('Markdown cannot say:')).toBeVisible();
  // Text and images are always in; attachments and comments are two ticks, both empty.
  await expect(dialog.getByRole('checkbox', { name: /Attachments/ })).not.toBeChecked();
  await expect(dialog.getByRole('checkbox', { name: /Comments/ })).not.toBeChecked();
  const download = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Export', exact: true }).click();
  const file = await download;
  expect(file.suggestedFilename()).toMatch(/\.md$/);
  const text = await readFile(await file.path(), 'utf8');
  expect(text).toContain('# Rollout plan');
  expect(text).toContain('Ship the redirect fix behind the flag');
  // What the export left out is said once the file is written.
  await expect(dialog.getByText(/What this format could not say:/)).toContainText('callout');
});

test('the other formats are asked for by name, and a PDF is made by the browser when the page holds no compiler: Print, which puts the dialog away and prints the page', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  // The dialog goes when the file has been written and nothing was left out; each format opens it again.
  const exportAs = async (format: string | RegExp) => {
    await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
    await page.getByRole('menuitem', { name: 'Export…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Export Rollout plan' });
    await dialog.getByRole('radio', { name: format }).check();
    return dialog;
  };
  const html = await exportAs('HTML');
  const htmlFile = page.waitForEvent('download');
  await html.getByRole('button', { name: 'Export', exact: true }).click();
  expect((await htmlFile).suggestedFilename()).toMatch(/\.html$/);
  const typst = await exportAs('Typst');
  const typstFile = page.waitForEvent('download');
  await typst.getByRole('button', { name: 'Export', exact: true }).click();
  // The source with its images is one archive.
  expect((await typstFile).suggestedFilename()).toMatch(/\.zip$/);
  // A PDF is the browser's own: the print dialog is its, and the page is printed once the dialog has gone from it.
  await page.evaluate(() => {
    // The browser fallback is still usable on a device without worker support.
    Object.defineProperty(window, 'Worker', { value: undefined, configurable: true });
    const counted = window as unknown as { printed: number };
    counted.printed = 0;
    window.print = () => {
      counted.printed += 1;
    };
  });
  // The source of Typst is said to be what a PDF is made from: the name of the choice is what it begins with.
  const pdf = await exportAs(/^PDF/);
  await expect(pdf.getByText(/A PDF is made by your browser/)).toBeVisible();
  await expect(pdf.getByRole('button', { name: 'Export', exact: true })).toHaveCount(0);
  await pdf.getByRole('button', { name: 'Print' }).click();
  await expect(pdf).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { printed: number }).printed)).toBe(1);
  // Print stays in the document's menu too.
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await expect(page.getByRole('menuitem', { name: 'Print' })).toBeVisible();
});

test('a PDF asked for from where the document is not open says it is printed from its page, and leads there', async ({ page }) => {
  await page.addInitScript(() => { Object.defineProperty(window, 'Worker', { value: undefined, configurable: true }); });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  // The document's row in the left bar, under its topic once that is open, has the same Export… as its page.
  await page.getByRole('tree').getByRole('treeitem', { name: /^Fix login/ }).press('ArrowRight');
  await page.getByRole('tree').getByRole('treeitem', { name: /Rollout plan/ }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Export Rollout plan' });
  await dialog.getByRole('radio', { name: /^PDF/ }).check();
  await expect(dialog.getByText('A PDF is printed from the page of the document. Open it, and choose Export… there.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Print' })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Open the document' }).click();
  await expect(page).toHaveURL(new RegExp(`/doc/${DOCS.plan}$`));
  await expect(dialog).toHaveCount(0);
});

test('a reader who may not write sees the page with no caret and no bars, and is told whose it is', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await openDocument(page, DOCS.plan);
  await expect(page.getByText('You can read this document. To write in it, ask Ada Lovelace, who owns it.')).toBeVisible();
  await expect(editorOf(page)).toHaveAttribute('contenteditable', 'false');
  await expect(page.getByRole('textbox', { name: 'Title' })).toHaveAttribute('readonly', '');
  await lineOf(page, 'Ship the redirect fix').dblclick();
  await page.keyboard.type('nothing');
  await expect(page.getByRole('toolbar', { name: 'Text formatting' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Block menu' })).toHaveCount(0);
  await expect(saveState(page)).toHaveCount(0);
  await page.waitForTimeout(1_500);
  const after = await held(DOCS.plan);
  expect(after.seq).toBe(1);
  expect(wordsOf(after.blocks.find((one) => one.id === block(2))!)).toBe('Ship the redirect fix behind the flag, then:');
  // Reading is whole: the film waits for a press and the formula is drawn.
  await expect(editorOf(page).locator('.katex').first()).toBeVisible();
  await expect(editorOf(page).locator('video')).toHaveCount(0);
});

test('a document is deleted, and brought back from the list of its place', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete', exact: true });
  await expect(dialog.getByText('Rollout plan.md', { exact: true }).last()).toBeVisible();
  await expect(dialog.getByText(/Recoverable until/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByText('Documents updated', { exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/inspect/document/${DOCS.plan}`));
  await expect(page.getByRole('heading', { name: 'Rollout plan', exact: true }).first()).toBeVisible();
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByTestId('main').getByRole('link', { name: 'Rollout plan', exact: true })).toHaveCount(0);
  const deleted = page.getByRole('button', { name: /^Recently deleted \(\d+\+?\)$/ });
  await expect(deleted).toHaveAttribute('aria-expanded', 'false');
  await deleted.click();
  await expect(deleted).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('listitem').filter({ has: page.getByText('Rollout plan', { exact: true }) }).getByRole('button', { name: /^Restore/ }).click();
  await expect(page.getByText('The document is back', { exact: true })).toBeVisible();
  await expect(page.getByTestId('main').getByRole('link', { name: 'Rollout plan', exact: true })).toBeVisible();
});

test('a document is made private, with who loses it said first, and opened again', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await expect(page.locator('[data-access-state]').filter({ hasText: 'Uses Fix login permissions' })).toBeVisible();
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Make private', exact: true }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Make this document private?' }).or(page.getByRole('dialog', { name: 'Make this document private?' }));
  await expect(dialog.getByText('Only its owners will be able to read it, until it is shared.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Make private' }).click();
  await expect(page.locator('[data-access-state]').filter({ hasText: 'Private' })).toBeVisible();
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Use place permissions', exact: true }).click();
  const again = page.getByRole('alertdialog', { name: 'Use place permissions for this document?' }).or(page.getByRole('dialog', { name: 'Use place permissions for this document?' }));
  await again.getByRole('button', { name: 'Use place permissions' }).click();
  await expect(page.locator('[data-access-state]').filter({ hasText: 'Uses Fix login permissions' })).toBeVisible();
});

test('a Markdown file is imported as a document whose blocks are what it says', async ({ page }) => {
  await signIn(page);
  await installImportFixture(page);
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await page.getByRole('tabpanel', { name: 'Documents', exact: true }).getByRole('button', { name: 'Import…', exact: true }).click();
  const importing = page.getByRole('dialog', { name: /^Import/ });
  await importing.getByTestId('people-import-files').setInputFiles({
    name: 'plan.md',
    mimeType: 'text/markdown',
    buffer: Buffer.from('# Imported plan\n\nA first line.\n\n- one\n- two\n\n- [x] done\n\n> said\n'),
  });
  await importing.getByRole('button', { name: 'Next', exact: true }).click();
  await importing.getByRole('button', { name: 'Next', exact: true }).click();
  await importing.getByRole('button', { name: 'Import', exact: true }).click();
  await importing.getByRole('link', { name: 'Imported plan', exact: true }).click();
  await page.locator('[data-reference-card][data-state="open"]').getByRole('button', { name: 'Open', exact: true }).click();
  await expect(page).toHaveURL(/\/doc\/[0-9a-f-]+/);
  await expect(page.getByRole('textbox', { name: 'Title' })).toHaveValue('Imported plan');
  await expect(editorOf(page).locator('[data-ot="bullet"]')).toHaveCount(2);
  await expect(editorOf(page).locator('[data-ot="todo"][data-checked="true"]')).toContainText('done');
  await expect(editorOf(page).locator('[data-ot="quote"]')).toContainText('said');
});
