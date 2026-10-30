/** People around the work: deliberate agent input review and unread marks survive ordinary interaction. */
import { expect, test } from '@playwright/test';

import { composer, messageSaying, openMessageMenu, sendMessage } from './chat-support.ts';
import { bodySent, ACME, scenario, signIn, threadHeld } from './support.ts';
import { DOCS, openDocument } from './documents-support.ts';

test.beforeEach(async () => { await scenario(); });

for (const access of ['private', 'follows'] as const) test(`a compact copy label states ${access} access and sends that choice`, async ({ page }) => {
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await page.getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Duplicate', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Start from another document' });
  await expect(dialog).toHaveAccessibleDescription('Your copy keeps text and files, without comments, history or the original access.');
  const follows = dialog.getByRole('checkbox', { name: 'Private copy off: destination access', exact: true });
  await expect(follows).not.toBeChecked();
  await expect(follows).not.toHaveAttribute('aria-describedby');
  await follows.focus();
  await page.keyboard.press('Space');
  const privateCopy = dialog.getByRole('checkbox', { name: 'Private copy: only you', exact: true });
  await expect(privateCopy).toBeChecked();
  await expect(dialog.getByText('The new document belongs to you. Only you can read it until you share it.')).toHaveCount(0);
  if (access === 'follows') await privateCopy.uncheck();
  const accepted = page.waitForResponse((response) => response.request().method() === 'POST' && /\/nodes\/[^/]+\/documents\/copy$/.test(new URL(response.url()).pathname));
  await dialog.getByRole('button', { name: 'Create document', exact: true }).click();
  const reply = await accepted;
  expect(reply.ok()).toBeTruthy();
  const body: unknown = (await bodySent(reply.request()));
  expect(body).toMatchObject({ source_document_id: DOCS.plan, access });
  await expect(dialog).toHaveCount(0);
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
