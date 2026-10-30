/** Find stays in one chat or document, and a result opens beside the conversation without taking its draft or return focus. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, emit, PEOPLE, playScene, scenario, signIn } from './support.ts';
import { userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { chats, composer } from './chat-support.ts';
import { actAs, block, caretAtEndOf, DOCS, editorOf, lineOf, openDocument } from './documents-support.ts';
import { id } from '../doubles/fixtures.ts';

test.beforeEach(async () => { await scenario(); });

async function findResultBeside(page: Page, words: string, result: string) {
  await page.keyboard.press('Control+k');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  const field = finder.getByRole('combobox', { name: 'Search terms', exact: true });
  await field.fill(`/ ${words}`);
  const row = finder.getByRole('option', { name: new RegExp(result) });
  await expect(row).toBeVisible();
  await row.click({ modifiers: ['Alt'] });
  await expect(page.getByRole('region', { name: 'Search result' })).toBeVisible();
}

test('chat Find accepts two letters, steps only this chat, and returns to the same unsent draft and caret', async ({ page }) => {
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'hi older match', at: '2026-10-01T10:00:00.000Z' });
  await chats('/say', { node: 'Fix login', as: 'grace', text: 'hi newer match', at: '2026-10-01T10:01:00.000Z' });
  await chats('/say', { node: 'Upgrade deps', as: 'ada', text: 'hi outside this chat' });
  const requests: URL[] = [];
  page.on('request', (request) => { const url = new URL(request.url()); if (url.pathname.endsWith('/search') && request.resourceType() === 'fetch') requests.push(url); });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  const draft = composer(page);
  await draft.fill('Keep this unsent message');
  await draft.evaluate((element: HTMLTextAreaElement) => element.setSelectionRange(4, 4));
  await draft.press('Control+f');
  const bar = page.getByRole('search', { name: 'Find in this chat' });
  const field = bar.getByRole('textbox');
  await expect(field).toBeFocused();
  await field.fill('hi');
  await expect(bar.getByRole('status')).toHaveText('1 of 2');
  await field.press('Enter');
  await expect(bar.getByRole('status')).toHaveText('2 of 2');
  await field.press('Shift+Enter');
  await expect(bar.getByRole('status')).toHaveText('1 of 2');
  const scoped = requests.filter((url) => url.searchParams.get('q') === 'hi');
  expect(scoped.length).toBeGreaterThan(0);
  expect(scoped.every((url) => url.searchParams.get('chat_id') !== null && url.searchParams.get('document_id') === null && url.searchParams.get('kinds') === 'chat_message')).toBe(true);
  await field.press('Escape');
  await expect(bar).toHaveCount(0);
  await expect(draft).toBeFocused();
  await expect(draft).toHaveValue('Keep this unsent message');
  expect(await draft.evaluate((element: HTMLTextAreaElement) => element.selectionStart)).toBe(4);
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
});

test('document Find searches only that document, steps its blocks, and restores the editor selection', async ({ page }) => {
  const first = id(8, 901);
  const second = id(8, 902);
  await actAs('ada', DOCS.plan, [
    { op: 'insert', block: { id: first, type: 'paragraph', content: [{ type: 'text', text: '查找 first match' }] }, parent_id: null, after_id: block(2) },
    { op: 'insert', block: { id: second, type: 'paragraph', content: [{ type: 'text', text: '查找 second match' }] }, parent_id: null, after_id: first },
  ]);
  await actAs('ada', DOCS.notes, [{ op: 'insert', block: { id: id(8, 903), type: 'paragraph', content: [{ type: 'text', text: '查找 outside this document' }] }, parent_id: null, after_id: null }]);
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await caretAtEndOf(page, '查找 first match');
  await editorOf(page).press('Control+f');
  const bar = page.getByRole('search', { name: 'Find in this document' });
  const field = bar.getByRole('textbox');
  await expect(field).toBeFocused();
  await field.fill('查找');
  await expect(bar.getByRole('status')).toHaveText('1 of 2');
  await field.press('Enter');
  await expect(bar.getByRole('status')).toHaveText('2 of 2');
  await field.press('Escape');
  await expect(editorOf(page)).toBeFocused();
  expect(await lineOf(page, '查找 first match').evaluate((element) => element.contains(element.ownerDocument.getSelection()?.anchorNode ?? null))).toBe(true);
  await page.keyboard.type(' kept');
  await expect(lineOf(page, '查找 first match')).toContainText('查找 first match kept');
  await expect(page).toHaveURL(new RegExp(`/doc/${DOCS.plan}$`));
});

test('a document result opens beside a thread, can be searched there, and returns to its untouched draft', async ({ page }) => {
  await playScene(ACME.mainThread, 'history');
  await actAs('ada', DOCS.plan, [{ op: 'insert', block: { id: id(8, 904), type: 'paragraph', content: [{ type: 'text', text: 'unique side document words' }] }, parent_id: null, after_id: block(2) }]);
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const draft = page.getByRole('textbox', { name: 'Message to the agent' });
  await draft.fill('Keep my agent question');
  await findResultBeside(page, 'unique side document', 'unique side document words');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  const result = page.getByRole('region', { name: 'Search result' });
  await expect(result.getByText('unique side document words')).toBeVisible();
  await result.getByRole('button', { name: 'Find in this document' }).click();
  const bar = result.getByRole('search', { name: 'Find in this document' });
  await bar.getByRole('textbox').fill('unique');
  await expect(bar.getByRole('status')).toHaveText('1 of 1');
  await expect(draft).toHaveValue('Keep my agent question');
  await result.getByRole('button', { name: 'Return to the conversation' }).click();
  await expect(result).toHaveCount(0);
  await expect(draft).toBeFocused();
  await expect(draft).toHaveValue('Keep my agent question');
});

test('a node-chat result opens at the actual message beside the thread and returns without navigating', async ({ page }) => {
  await playScene(ACME.mainThread, 'history');
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'unique side chat words' });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const draft = page.getByRole('textbox', { name: 'Message to the agent' });
  await draft.fill('An unsent conversation');
  await findResultBeside(page, 'unique side chat', 'unique side chat words');
  const result = page.getByRole('region', { name: 'Search result' });
  await expect(result.locator('li[data-message]', { hasText: 'unique side chat words' })).toBeVisible();
  await result.getByRole('button', { name: 'Return to the conversation' }).click();
  await expect(draft).toBeFocused();
  await expect(draft).toHaveValue('An unsent conversation');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
});

test('another conversation opens at the hit beside this one and returns to the same question', async ({ page }) => {
  await playScene(ACME.mainThread, 'history');
  await playScene(ACME.repro, 'history');
  await emit(ACME.repro, { items: [userMessage('side-find-thread', 'unique separate conversation words', { author: ADA })] });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const draft = page.getByRole('textbox', { name: 'Message to the agent' });
  await draft.fill('Keep this main question');
  await findResultBeside(page, 'unique separate conversation', 'unique separate conversation words');
  const result = page.getByRole('region', { name: 'Search result' });
  await expect(result.getByRole('log', { name: 'Conversation' })).toContainText('unique separate conversation words');
  await result.getByRole('button', { name: 'Find in the conversation' }).click();
  const bar = result.getByRole('search', { name: 'Find in the conversation' });
  await bar.getByRole('textbox').fill('unique separate');
  await expect(bar.getByRole('status')).toHaveText('1 of 1');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  await result.getByRole('button', { name: 'Return to the conversation' }).click();
  await expect(draft).toBeFocused();
  await expect(draft).toHaveValue('Keep this main question');
});

test('a private document is absent from search and cannot be read by its address', async ({ page }) => {
  await actAs('ada', DOCS.private, [{ op: 'insert', block: { id: id(8, 905), type: 'paragraph', content: [{ type: 'text', text: 'private unique finding words' }] }, parent_id: null, after_id: null }]);
  await signIn(page, PEOPLE.vera);
  await page.goto('/search?q=private%20unique');
  await expect(page.getByText('Nothing found for “private unique”')).toBeVisible();
  await expect(page.getByRole('region', { name: 'Results' }).getByText('private unique finding words')).toHaveCount(0);
  await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname.includes(`/documents/${DOCS.private}`) && [403, 404].includes(response.status())),
    page.goto(`/doc/${DOCS.private}`),
  ]);
  await expect(editorOf(page)).toHaveCount(0);
  await expect(page.getByText('private unique finding words')).toHaveCount(0);
});
