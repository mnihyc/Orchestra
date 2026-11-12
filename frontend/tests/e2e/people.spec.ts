/** People around the work: deliberate agent input review and unread marks survive ordinary interaction. */
import { expect, test } from '@playwright/test';

import { composer, messageSaying, openMessageMenu, sendMessage } from './chat-support.ts';
import { bodySent, ACME, scenario, signIn, threadHeld } from './support.ts';
import { DOCS, openDocument } from './documents-support.ts';

test.beforeEach(async () => { await scenario(); });

for (const access of ['private', 'follows'] as const) test(`a native document copy retains ${access} access through current controls`, async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  const sourcePath = new URL(page.url()).pathname;
  const accepted = page.waitForResponse((response) => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/api/v1/document-tree/operations');
  await page.locator('[data-main-area]').getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
  // Duplicate is now the immediate native copy flow; the old preliminary access checkbox is gone.
  await page.getByRole('menuitem', { name: 'Duplicate', exact: true }).click();
  const reply = await accepted;
  expect(reply.ok()).toBe(true);
  const authorization = reply.request().headers().authorization;
  if (authorization === undefined) throw new Error('The real copy request must carry this test reader');
  const copyBody: unknown = await bodySent(reply.request());
  expect(copyBody).toMatchObject({ command: 'copy', items: [{ kind: 'document', id: DOCS.plan }] });
  expect(copyBody).not.toHaveProperty('access');
  await expect.poll(() => new URL(page.url()).pathname).not.toBe(sourcePath);
  const copiedId = new URL(page.url()).pathname.split('/').filter(Boolean).at(-1);
  if (copiedId === undefined || copiedId === DOCS.plan) throw new Error('The copy must have a fresh document identity');
  const metadata = async (id: string): Promise<unknown> => {
    const response = await page.request.get(`/api/v1/documents/${id}`, { headers: { authorization } });
    expect(response.ok()).toBe(true);
    const value: unknown = await response.json();
    return value;
  };
  await expect.poll(() => metadata(copiedId)).toMatchObject({ id: copiedId, title: 'Rollout plan', page: 'follows' });
  await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue('Rollout plan');
  if (access === 'private') {
    // Privacy is a deliberate action on the new document after the native copy has completed.
    await page.locator('[data-main-area]').getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Make private', exact: true }).click();
    const dialog = page.getByRole('alertdialog', { name: 'Make this document private?', exact: true })
      .or(page.getByRole('dialog', { name: 'Make this document private?', exact: true }));
    await expect(dialog).toContainText('Only its owners will be able to read it, until it is shared.');
    const changed = page.waitForResponse((response) => response.request().method() === 'PATCH'
      && new URL(response.url()).pathname === `/api/v1/documents/${copiedId}/permissions`);
    await dialog.getByRole('button', { name: 'Make private', exact: true }).click();
    const answer = await changed;
    expect(answer.ok()).toBe(true);
    expect(await bodySent(answer.request())).toMatchObject({ whole: 'private' });
    await expect(dialog).toHaveCount(0);
    await expect.poll(() => metadata(copiedId)).toMatchObject({ page: 'restricted' });
  }
  await expect.poll(() => metadata(DOCS.plan)).toMatchObject({ id: DOCS.plan, page: 'follows' });
});

test('a chosen agent previews its actual forwarded input before one visible send, and editing never asks again', async ({page}) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  const before = (await threadHeld(ACME.mainThread)).inputs.length;
  await composer(page).fill('Please @Main');
  await page.getByRole('option',{name:/Main thread/}).click();
  await expect(page.getByText('Queues as your input in Main thread and counts as your use.')).toBeVisible();
  await expect(page.getByText('Please @Main thread').first()).toBeVisible();
  expect((await threadHeld(ACME.mainThread)).inputs).toHaveLength(before);
  await page.getByRole('button',{name:'Send message',exact:true}).click();
  const visible = messageSaying(page,'Please @Main thread');
  await expect(visible.getByRole('link',{name:'Asked in Main thread'})).toHaveAttribute('href',`/thread/${ACME.mainThread}`);
  const accepted = (await threadHeld(ACME.mainThread)).inputs;
  expect(accepted).toHaveLength(before+1);
  expect(accepted.at(-1)).toMatchObject({delivery:'queue',content:[{type:'forwarded',text:'Please @Main thread'}]});
  await openMessageMenu(page,'Please @Main thread');
  await page.getByRole('menuitem',{name:'Edit',exact:true}).click();
  await page.getByRole('textbox',{name:'Edit message'}).fill('Please @Main thread review the correction.');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await expect(messageSaying(page,'review the correction.')).toContainText('(edited)');
  expect((await threadHeld(ACME.mainThread)).inputs).toHaveLength(before+1);
});

test('marking a live message unread stays visible while this chat is open and its search retains the chat scope', async ({page}) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await sendMessage(page,'Canary observation for tomorrow');
  await openMessageMenu(page,'Canary observation for tomorrow');
  await page.getByRole('menuitem',{name:'Mark unread from here'}).click();
  await expect(page.getByRole('status').filter({hasText:'Marked unread. It stays unread until you reopen this chat.'})).toBeVisible();
  await composer(page).fill('A draft does not consume that mark');
  await expect(page.getByText('Marked unread. It stays unread until you reopen this chat.')).toBeVisible();
  await expect(page.getByRole('button',{name:'Find in this chat',exact:true})).toBeVisible();
  const history=page.getByRole('button',{name:'Search chat history',exact:true});
  await expect(history.locator('[data-action-icon="searchHistory"]')).toHaveCount(1);
  await history.click();
  await expect(page).toHaveURL(/\/search\?/);
  const params = new URL(page.url()).searchParams;
  expect(params.get('scope')).toBe(ACME.fixLogin);
  expect(params.get('kinds')).toBe('chat_message');
  expect(params.get('chat_id')).toBeTruthy();
});
