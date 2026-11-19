/** Documents: a page of blocks read, a document made, a block of every kind written and read back, and two people writing in one document.
 *
 * Against the fake API and its fake socket: the other person is a second browser, or a request that
 * plays them. What a test asserts about the server is what the fake server holds, in the order of the page.
 */
import { expect, test } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';
import {
  actAs,
  block,
  caretAtEndOf,
  DOCS,
  editorOf,
  held,
  lineOf,
  openDocument,
  saveState,
  slash,
  waitUntilSaved,
  wordsOf,
} from './documents-support.ts';

test.beforeEach(async () => {
  await scenario();
});

test('a page shows a block of every one of the twenty kinds as it is stored', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  const page_ = editorOf(page);
  const has = (selector: string) => expect(page_.locator(selector).first(), selector).toBeVisible();
  for (const kind of ['paragraph', 'heading', 'bullet', 'numbered', 'todo', 'toggle', 'quote', 'callout', 'codeBlock', 'math', 'diagram', 'divider', 'pageBreak', 'image', 'video', 'audio', 'file']) {
    await has(`[data-ot="${kind}"]`);
  }
  await has('table');
  await has('.ob-columns');
  await has('.ob-column');
  // The words are the stored words: a mark, a tick, a nested list, a tone.
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText('Ship the redirect fix behind the flag, then:');
  await expect(page_.locator('strong', { hasText: 'then' })).toBeVisible();
  await expect(page_.locator('[data-ot="todo"][data-checked="true"]')).toHaveCount(1);
  await expect(page_.locator('[data-ot="callout"][data-tone="warning"]')).toContainText('Do not enable before the migration');
  await expect(page_.locator('[data-ot="numbered"]').first()).toContainText('Disable the flag');
  await expect(page_.locator('[data-ot="heading"][data-level="1"]')).toContainText('Rollout plan');
  // A formula is drawn as mathematics, and a film is not played until the reader chooses.
  await expect(page_.locator('.katex').first()).toBeVisible();
  await expect(page_.getByRole('button', { name: 'Play video from media.example.org', exact: true })).toBeVisible();
  await expect(page_.locator('video')).toHaveCount(0);
  // The title and the line under it.
  await expect(page.getByRole('textbox', { name: 'Title' })).toHaveValue('Rollout plan');
  await expect(page.getByText('Ada Lovelace changed this')).toBeVisible();
  await expect(page.locator('[data-access-state]').filter({ hasText: 'Uses Fix login permissions' })).toBeVisible();
  // Opening a page writes nothing.
  expect((await held(DOCS.plan)).seq).toBe(1);
});

test('one press makes a document and opens its title, then Enter writes its body and the page keeps both', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByRole('link', { name: 'Rollout plan' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Notes from the canary' })).toBeVisible();
  await page.getByRole('button', { name: 'New document', exact: true }).click();
  await expect(page).toHaveURL(/\/doc\/[0-9a-f-]+$/);
  const title = page.getByRole('textbox', { name: 'Title' });
  await expect(title).toBeFocused();
  await expect(page.getByRole('textbox', { name: 'Name', exact: true })).toHaveCount(0);
  // The selected Untitled value is replaced on the page itself, without a name popover or Create.
  await page.keyboard.type('Launch checklist');
  await page.keyboard.press('Enter');
  await expect(title).toHaveValue('Launch checklist');
  await expect(page.getByRole('textbox', { name: 'Document text' })).toBeFocused();
  await page.keyboard.type('First thing to do');
  await waitUntilSaved(page);
  const url = new URL(page.url());
  const id = url.pathname.split('/').pop() ?? '';
  const made = await held(id);
  await expect.poll(async () => (await held(id)).title).toBe('Launch checklist');
  expect(made.blocks.map((one) => [one.type, wordsOf(one)])).toEqual([['paragraph', 'First thing to do']]);
  // A reload does not move the cursor again.
  await page.reload();
  await expect(title).toHaveValue('Launch checklist');
  await expect(title).not.toBeFocused();
  await expect(page.getByRole('textbox', { name: 'Document text' })).not.toBeFocused();
  // It is in the list of its place, newest change first.
  await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
  await expect(page.getByRole('link', { name: 'Launch checklist' })).toBeVisible();
});

test('what is typed is sent once, as one change to one block, and shown as Saving until it is answered', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' today');
  // Within a tenth of a second the header says what is under way.
  await expect(saveState(page)).toHaveAttribute('data-save', 'saving', { timeout: 500 });
  await waitUntilSaved(page);
  const after = await held(DOCS.plan);
  expect(after.seq).toBe(2);
  const changed = after.blocks.filter((one) => one.seq_updated === 2);
  expect(changed.map((one) => one.id)).toEqual([block(2)]);
  expect(wordsOf(changed[0]!)).toBe('Ship the redirect fix behind the flag, then: today');
  // A reload shows the same page.
  await page.reload();
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText('then: today');
});

test('Enter ends a block and starts the next of its kind, Backspace turns it back into text, Tab puts it under the one above', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'A bulleted line');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Second bullet');
  await page.keyboard.press('Tab');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Third, nested');
  await waitUntilSaved(page);
  let now = await held(DOCS.plan);
  const first = now.blocks.find((one) => wordsOf(one) === 'A bulleted line')!;
  const second = now.blocks.find((one) => wordsOf(one) === 'Second bullet')!;
  const third = now.blocks.find((one) => wordsOf(one) === 'Third, nested')!;
  expect([first.type, second.type, third.type]).toEqual(['bullet', 'bullet', 'bullet']);
  expect(second.parent_id).toBe(first.id);
  expect(third.parent_id).toBe(first.id);
  // Shift+Tab takes it out again, and Backspace at the start of a bullet makes it a paragraph.
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Home');
  await page.keyboard.press('Backspace');
  await waitUntilSaved(page);
  now = await held(DOCS.plan);
  const former = now.blocks.find((one) => wordsOf(one) === 'Third, nested')!;
  expect(former.type).toBe('paragraph');
  expect(former.parent_id).toBeNull();
});

test('the block menu opens at a slash, narrows as it is typed, and every kind it offers is written and read back', async ({ page }) => {
  await signIn(page);
  const id = await (await import('./documents-support.ts')).makeLong(0, 'Every kind');
  await openDocument(page, id);
  await editorOf(page).locator('p.oc').first().click();
  await page.keyboard.type('A plain paragraph');
  await page.keyboard.press('Enter');
  // The menu narrows as it is typed, and offers only what can be made: an image waits for a place to keep it in.
  await page.keyboard.type('/ima');
  const list = page.getByRole('listbox', { name: 'Blocks' });
  await expect(list.getByText('Nothing matches')).toBeVisible();
  await expect(list.getByRole('option', { name: /Image/ })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');

  // Each block is made as a person makes it: a slash and its name, its words, and the key that ends it (Enter; Mod+Enter in code).
  const write = async (query: string, words?: string, leave?: string) => {
    await slash(page, query);
    if (words !== undefined) await page.keyboard.type(words);
    if (leave !== undefined) await page.keyboard.press(leave);
  };
  await write('heading 2', 'A heading', 'Enter');
  await write('bulleted', 'A bullet', 'Enter');
  await write('numbered', 'A number', 'Enter');
  await write('to-do', 'A task', 'Enter');
  await write('toggle', 'A toggle', 'Enter');
  await write('quote', 'A quote', 'Enter');
  await write('callout', 'A callout', 'Enter');
  await write('code', 'let x = 1;', 'Control+Enter');
  await write('divider');
  await write('page break');
  await write('table', 'cell', 'Control+End');
  await write('3 columns', 'in a column', 'Control+End');
  await write('formula', 'x^2+y^2', 'Escape');
  await write('diagram', 'graph TD; A-->B', 'Escape');
  await waitUntilSaved(page);

  const types = new Set((await held(id)).blocks.map((one) => one.type));
  for (const type of ['paragraph', 'heading', 'bullet', 'numbered', 'todo', 'toggle', 'quote', 'callout', 'code', 'divider', 'page_break', 'table', 'columns', 'column', 'math', 'diagram']) {
    expect(types, type).toContain(type);
  }
  const now = await held(id);
  expect(now.blocks.find((one) => one.type === 'heading')?.props.level).toBe(2);
  expect(wordsOf(now.blocks.find((one) => one.type === 'code')!)).toBe('let x = 1;');
  expect(now.blocks.find((one) => one.type === 'math')?.props.source).toBe('x^2+y^2');
  expect(now.blocks.filter((one) => one.type === 'column')).toHaveLength(3);
  // A reload reads the same page back.
  await page.reload();
  await expect(editorOf(page).locator('[data-ot="callout"]')).toContainText('A callout');
  await expect(editorOf(page).locator('table')).toBeVisible();
  await expect(editorOf(page).locator('.ob-column')).toHaveCount(3);
  await expect(editorOf(page).locator('.katex').first()).toBeVisible();
});

test('a film is given as an address and is played from there only when the reader chooses', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'A bulleted line');
  await page.keyboard.press('Enter');
  await slash(page, 'film');
  const dialog = page.getByRole('dialog', { name: 'Add a film' });
  await dialog.getByRole('textbox', { name: 'Address' }).fill('http://insecure.example.org/a.mp4');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog.getByText('Give an address that begins with https://')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Address' }).fill('https://films.example.org/new.mp4');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(editorOf(page).getByText('https://films.example.org/new.mp4')).toBeVisible();
  await expect(editorOf(page).locator('video')).toHaveCount(0);
  await waitUntilSaved(page);
  const stored = (await held(DOCS.plan)).blocks.find((one) => one.props.url === 'https://films.example.org/new.mp4');
  expect(stored?.type).toBe('video');
});

test('two people writing different blocks see each other\'s words arrive, and the writer is shown beside the block', async ({ page, browser }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  // Dora, a developer, has the same document open in a page of her own.
  const doraContext = await browser.newContext();
  const dora = await doraContext.newPage();
  await signIn(dora, PEOPLE.dora);
  await openDocument(dora, DOCS.plan);

  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' (Ada was here)');
  await caretAtEndOf(dora, 'A bulleted line');
  await dora.keyboard.type(' (Dora was here)');
  await waitUntilSaved(page);
  await waitUntilSaved(dora);

  // Each sees the other's words, and neither's text was disturbed.
  await expect(lineOf(page, 'A bulleted line')).toContainText('A bulleted line (Dora was here)');
  await expect(lineOf(dora, 'Ship the redirect fix')).toContainText('then: (Ada was here)');
  const now = await held(DOCS.plan);
  expect(now.blocks.map(wordsOf)).toContain('A bulleted line (Dora was here)');
  expect(now.blocks.map(wordsOf).some((words) => words.endsWith('(Ada was here)'))).toBe(true);
  // The person who wrote is stood beside the block for a moment, on the other's page.
  await expect(page.getByRole('img', { name: 'Dora Developer is writing here' }).first()).toBeVisible();
  // In the page of the document: the chat of the topic beside it says the same of itself.
  await expect(page.getByTestId('main').getByLabel('Looking now: Dora Developer')).toBeVisible();
  await doraContext.close();
});

test('two people writing in the same block: the second finds the first\'s words standing and their own kept under them, with three choices', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' MINE');
  // Grace's change reaches the server before Ada's is sent.
  await actAs('grace', DOCS.plan, [
    { op: 'update', id: block(2), base_seq: 1, content: [{ type: 'text', text: 'Ship it the other way, GRACES' }] },
  ]);
  const kept = page.getByRole('group', { name: 'Your text, kept beside another change' });
  await expect(kept).toBeVisible();
  await expect(kept).toContainText('Grace Hopper changed this while you were writing. Yours is kept here.');
  await expect(lineOf(page, 'Ship it the other way')).toContainText('GRACES');
  await expect(lineOf(page, 'MINE')).toContainText('Ship the redirect fix behind the flag, then: MINE');
  await waitUntilSaved(page);
  // Both are on the server, the other's first, and nothing was lost.
  let now = await held(DOCS.plan);
  const words = now.blocks.map(wordsOf);
  expect(words.indexOf('Ship it the other way, GRACES')).toBeGreaterThan(-1);
  expect(words.indexOf('Ship the redirect fix behind the flag, then: MINE')).toBe(words.indexOf('Ship it the other way, GRACES') + 1);

  // Keep both: the notice goes and both stay.
  await kept.getByRole('button', { name: 'Keep both' }).click();
  await expect(kept).toHaveCount(0);
  await waitUntilSaved(page);
  now = await held(DOCS.plan);
  expect(now.blocks.map(wordsOf)).toContain('Ship the redirect fix behind the flag, then: MINE');
});

test('the change that came second can replace the other\'s, or be discarded', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' ONE');
  await actAs('grace', DOCS.plan, [{ op: 'update', id: block(2), base_seq: 1, content: [{ type: 'text', text: 'Theirs, first' }] }]);
  const kept = page.getByRole('group', { name: 'Your text, kept beside another change' });
  await kept.getByRole('button', { name: "Replace Grace Hopper's with mine" }).click();
  await expect(kept).toHaveCount(0);
  await waitUntilSaved(page);
  let now = await held(DOCS.plan);
  expect(now.blocks.map(wordsOf)).not.toContain('Theirs, first');
  expect(now.blocks.find((one) => one.id === block(2))?.content.map((run) => run.text).join('')).toBe('Ship the redirect fix behind the flag, then: ONE');

  // The other way: a second conflict, discarded.
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' TWO');
  const seq = (await held(DOCS.plan)).blocks.find((one) => one.id === block(2))!.seq_updated;
  await actAs('grace', DOCS.plan, [{ op: 'update', id: block(2), base_seq: seq, content: [{ type: 'text', text: 'Theirs, second' }] }]);
  await expect(kept).toBeVisible();
  await kept.getByRole('button', { name: 'Discard mine' }).click();
  await expect(kept).toHaveCount(0);
  await waitUntilSaved(page);
  now = await held(DOCS.plan);
  expect(now.blocks.map(wordsOf)).toContain('Theirs, second');
  expect(now.blocks.map(wordsOf).some((words) => words.endsWith('TWO'))).toBe(false);
});

test('Markdown pasted becomes the blocks it says, in place of an empty line, and is sent as blocks', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Do not enable before the migration');
  await page.keyboard.press('Enter');
  // Plain text that says Markdown arrives as the text a clipboard gives.
  const markdown = ['## Pasted heading', '', '- first', '  - nested **bold**', '- [x] ticked', '', '| Step | Owner |', '| --- | --- |', '| Canary | Dora |', '', '```ts', 'let pasted = 1;', '```'].join('\n');
  await page.evaluate((text) => {
    const target = document.querySelector('.doc-editor');
    const data = new DataTransfer();
    data.setData('text/plain', text);
    target?.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  }, markdown);
  const page_ = editorOf(page);
  await expect(page_.locator('[data-ot="heading"]', { hasText: 'Pasted heading' })).toHaveAttribute('data-level', '2');
  await expect(page_.locator('[data-ot="bullet"]', { hasText: 'nested' })).toBeVisible();
  await expect(page_.locator('[data-ot="bullet"] strong', { hasText: 'bold' })).toBeVisible();
  await expect(page_.locator('[data-ot="todo"][data-checked="true"]', { hasText: 'ticked' })).toBeVisible();
  // The page had one table; the pasted one is the second, with its header row.
  await expect(page_.locator('table')).toHaveCount(2);
  await expect(page_.locator('table').last()).toContainText('Canary');
  await expect(page_.locator('[data-ot="codeBlock"]', { hasText: 'let pasted = 1;' })).toBeVisible();
  await waitUntilSaved(page);
  const now = await held(DOCS.plan);
  const heading = now.blocks.find((one) => wordsOf(one) === 'Pasted heading');
  const first = now.blocks.find((one) => wordsOf(one) === 'first');
  const nested = now.blocks.find((one) => wordsOf(one) === 'nested bold');
  expect(heading?.type).toBe('heading');
  expect(heading?.props.level).toBe(2);
  expect(nested?.parent_id).toBe(first?.id);
  expect(nested?.content.map((run) => (run as { marks?: string[] }).marks ?? [])).toEqual([[], ['bold']]);
  expect(now.blocks.find((one) => one.type === 'code' && wordsOf(one) === 'let pasted = 1;')?.props.language).toBe('ts');
  expect(now.blocks.find((one) => one.type === 'table' && one.props.header_row === true)).toBeDefined();
  // The empty line the caret was in is where they went: no empty paragraph is left behind the callout.
  expect(now.blocks.filter((one) => one.type === 'paragraph' && wordsOf(one) === '')).toHaveLength(0);
});

test('a bold word pasted goes into the line the caret is in, and plain words are pasted as plain words', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, 'Ship the redirect fix');
  const paste = (text: string) =>
    page.evaluate((value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value);
      document.querySelector('.doc-editor')?.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
    }, text);
  await paste(' **now**');
  await expect(lineOf(page, 'Ship the redirect fix').locator('strong', { hasText: 'now' })).toBeVisible();
  await paste(' a * b and c_d');
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText('then: now a * b and c_d');
  await waitUntilSaved(page);
  const now = await held(DOCS.plan);
  expect(now.blocks.filter((one) => one.seq_updated > 1).map((one) => one.id)).toEqual([block(2)]);
});

test('a change that cannot be sent is kept and said to be not saved yet, and goes by itself when the connection is back', async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  let down = true;
  await page.route('**/documents/*/ops', (route) => (down ? route.abort('connectionrefused') : route.continue()));
  await caretAtEndOf(page, 'Ship the redirect fix');
  await page.keyboard.type(' later');
  await expect(saveState(page)).toHaveAttribute('data-save', 'offline');
  await expect(saveState(page)).toHaveText('Not saved yet · reconnecting');
  // Nothing was lost, and nothing reached the server.
  await expect(lineOf(page, 'Ship the redirect fix')).toContainText('then: later');
  expect(wordsOf((await held(DOCS.plan)).blocks.find((one) => one.id === block(2))!)).toBe('Ship the redirect fix behind the flag, then:');
  // Typing goes on meanwhile, and leaving the page is asked about.
  await page.keyboard.type(' and more');
  down = false;
  await waitUntilSaved(page);
  expect(wordsOf((await held(DOCS.plan)).blocks.find((one) => one.id === block(2))!)).toBe('Ship the redirect fix behind the flag, then: later and more');
});
