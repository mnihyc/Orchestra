/** Every state of people's chats, asked in both themes whether anything on it is a serious barrier: a conversation, its menus and pickers, the Messages screen, the dialogs, a selection, a guest's view, and a small screen. */
import { expect, type Page, test } from '@playwright/test';
import { showWorkbenchView } from '../e2e/workbench-support.ts';
import { asAdmin } from '../e2e/admin-mode-support.ts';

import { chats, composer, messageSaying, openMessageMenu } from '../e2e/chat-support.ts';
import { ACME, expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

const TOPIC = `/topic/${ACME.fixLogin}`;
const MESSAGES = '/org/acme/messages';

/** The first message that says it: a reply repeats the first line of what it answers. */
const said = (page: Page, text: string) => messageSaying(page, text).first();

test.beforeEach(async () => {
  await scenario();
});

test('a topic’s conversation with its replies, mentions and code, beside the topic, and the people typing in it', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await expect(said(page, 'Has anybody seen the session cookie')).toBeVisible();
  await expect(said(page, 'Thanks @linus')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await chats('/typing', { node: 'Fix login', as: 'dora' });
  await expect(page.getByText('Dora Developer is typing…')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the actions of a message, in the menu that opens from it', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await expect(said(page, 'Please take a look')).toBeVisible();

  await openMessageMenu(page, 'Please take a look');
  await expect(page.getByRole('menu', { name: /^Actions/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the emoji picker of a reaction, and the message with the reaction made', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  const message = said(page, 'Please take a look');
  await message.hover();
  await message.getByRole('button', { name: 'React' }).click();
  await expect(page.getByRole('group', { name: 'Faces' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'slight smile' }).click();
  await expect(message.getByRole('button', { name: /slight smile|🙂/ }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the people to mention that open over the box a message is written in', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await composer(page).click();
  await composer(page).pressSequentially('Thanks @li');
  await expect(page.getByRole('listbox', { name: 'People to mention' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the notifications of a chat and the messages pinned in it', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await openMessageMenu(page, 'Please take a look');
  await page.getByRole('menuitem', { name: 'Pin' }).click();
  await expect(said(page, 'Please take a look').getByText('Pinned by Ada Lovelace')).toBeVisible();

  await page.getByRole('button', { name: /^You have not chosen here/ }).click();
  await expect(page.getByRole('menuitemradio', { name: 'All', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Pinned messages' }).click();
  await expect(page.getByRole('dialog', { name: 'Pinned messages' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a message edited in place, one that is not sent yet, and the sentence that says why a chat cannot be written in', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await openMessageMenu(page, 'Thanks @linus');
  await page.getByRole('menuitem', { name: 'Edit' }).click();
  await expect(page.getByRole('textbox', { name: 'Edit message' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // A chat that is switched off for its place, and one whose place was archived.
  await chats('/switch', { node: 'Fix login', off: true });
  await page.reload();
  await expect(page.getByText('Chats are off for this topic.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the Messages screen: its list, the unread counts, and a conversation open beside it', async ({ page }) => {
  const direct = await chats('/chat', { org: ACME.org, members: ['dora', 'ada'] });
  await chats('/say', { chat: direct.chat, as: 'dora', text: 'Have you got a minute?' });
  const group = await chats('/chat', { org: ACME.org, members: ['linus', 'ada', 'dora'], name: 'Release crew' });
  await chats('/say', { chat: group.chat, as: 'linus', text: '@ada the build is green' });
  await signIn(page);
  await page.goto(MESSAGES);
  await expect(page.getByRole('list', { name: 'Chats' }).getByRole('listitem')).toHaveCount(2);
  await expectAccessibleInBothThemes(page);

  await page.getByRole('list', { name: 'Chats' }).getByRole('link', { name: /Release crew/ }).click();
  await expect(said(page, 'the build is green')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the menu of one chat in the list, and the menu of a group', async ({ page }) => {
  const group = await chats('/chat', { org: ACME.org, members: ['linus', 'ada', 'dora'], name: 'Release crew' });
  await chats('/say', { chat: group.chat, as: 'linus', text: '@ada the build is green' });
  await signIn(page);
  await page.goto(`${MESSAGES}?chat=${String(group.chat)}`);
  await expect(said(page, 'the build is green')).toBeVisible();

  await page.getByRole('button', { name: /^Actions for / }).first().click();
  await expect(page.getByRole('menuitem', { name: 'Mute' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'People in this group' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the people of a group, and adding somebody from outside the org as a guest', async ({ page }) => {
  await chats('/guest', { username: 'gus' });
  const group = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Release crew' });
  await signIn(page);
  await page.goto(`${MESSAGES}?chat=${String(group.chat)}`);
  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await page.getByRole('menuitem', { name: 'People in this group' }).click();
  await expect(page.getByRole('dialog', { name: 'People in this group' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Add people…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add people', exact: true });
  await dialog.getByRole('searchbox', { name: 'Search people' }).fill('gus');
  await expect(dialog.getByRole('button', { name: /Gus Guest/ })).toBeVisible();
  await dialog.getByRole('button', { name: /Gus Guest/ }).click();
  await expectAccessibleInBothThemes(page);
});

test('New message: choosing people for a direct chat, and for a group with its name', async ({ page }) => {
  await signIn(page);
  await page.goto(MESSAGES);
  await expect(page.getByText('Talk to a teammate directly.')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'New message' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'New message' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /Dora Developer/ }).click();
  await expect(dialog.getByRole('button', { name: 'Message Dora Developer' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: /Linus Pauling/ }).click();
  await expect(dialog.getByRole('button', { name: 'Start a group of 2' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('an admin who reads the chats of others and of places, with the sentence that says it is silent', async ({ page }) => {
  await chats('/chat', { org: ACME.org, members: ['dora', 'linus'], name: 'Docs crew' });
  await chats('/say', { node: 'Fix login', as: 'dora', text: 'A word on this topic' });
  await signIn(page);
  await asAdmin(page, true);
  await page.goto(MESSAGES);
  await expect(page.getByRole('switch', { name: /Show all chats/ })).toHaveCount(0);
  await page.getByText("Other people's chats", { exact: true }).click();
  await expect(page.getByRole('link', { name: 'Docs crew' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('link', { name: 'Docs crew' }).click();
  await expect(page.getByRole('img', { name: 'You see this as an admin of Acme' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('messages picked in a selection, and the dialog that forwards them', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto(TOPIC);
  await openMessageMenu(page, 'I will write up what I saw');
  await page.getByRole('menuitem', { name: 'Select' }).click();
  await expect(page.getByRole('toolbar', { name: 'Selected messages' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('checkbox', { name: /Select the message of Dora Developer/ }).click();
  await page.getByRole('button', { name: 'Forward…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Forward' });
  await expect(dialog).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('searchbox', { name: 'Search chats and people' }).fill('li');
  await dialog.getByRole('button', { name: /Linus Pauling/ }).click();
  await expect(dialog.getByRole('button', { name: 'Forward to 1 place' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a guest of a group, who sees that chat and nothing else of the org', async ({ page }) => {
  await chats('/guest', { username: 'gus' });
  const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Launch crew', guests: ['gus'] });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Welcome aboard, Gus' });
  await signIn(page, { login: 'gus', password: 'a guest has a long password' });
  await page.goto(`${MESSAGES}?chat=${String(made.chat)}`);
  await expect(said(page, 'Welcome aboard, Gus')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a thread’s chat beside it with its three segments, and the same on a small screen', async ({ page }) => {
  await chats('/seed');
  await signIn(page);
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Chat');
  const segments = page.getByRole('radiogroup', { name: 'Chats of this place' });
  await expect(segments.getByRole('radio', { name: /Thread/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto(MESSAGES);
  await expect(page.getByRole('heading', { name: 'Messages' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the conversation and the Messages screen in Chinese', async ({ page }) => {
  await chats('/seed');
  const group = await chats('/chat', { org: ACME.org, members: ['linus', 'ada', 'dora'], name: 'Release crew' });
  await chats('/say', { chat: group.chat, as: 'linus', text: '@ada the build is green' });
  await chats('/locale', { as: 'ada', locale: 'zh-CN' });
  await signIn(page);
  await page.goto(TOPIC);
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.locator('li[data-message]').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto(`${MESSAGES}?chat=${String(group.chat)}`);
  await expect(page.locator('li[data-message]').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
