/** People's chats in a real browser: a message sent and seen by a second person, an edit and a delete shown as such, a reaction, a pin, a reply, who is typing, unread counts that fall as the reader reads, a direct chat made, a guest who sees the chat and nothing else of the org, an admin reading what is not theirs, and messages selected and forwarded to a chat and a person. */
import { expect, test } from '@playwright/test';
import { showWorkbenchView } from './workbench-support.ts';

import { chats, composer, messageSaying, openMessageMenu, secondReader, sendMessage } from './chat-support.ts';
import { ACME, PEOPLE, playScene, scenario, signIn } from './support.ts';
import { asAdmin } from './admin-mode-support.ts';

const TOPIC = `/topic/${ACME.fixLogin}`;
const MESSAGES = '/org/acme/messages';

test.beforeEach(async () => {
  await scenario();
});

test('a message sent in a topic’s chat is seen by a second reader as it arrives, under its author, in order', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  await expect(page.getByRole('radiogroup', { name: 'Chats of this place' })).toBeVisible();
  await expect(page.getByText('No messages yet. Say something to the people on this topic.')).toBeVisible();

  const second = await secondReader(browser, baseURL, TOPIC);
  await sendMessage(page, 'The session ends after ten minutes.');
  await expect(messageSaying(second.page, 'The session ends after ten minutes.')).toBeVisible();
  await expect(messageSaying(second.page, 'The session ends after ten minutes.').getByText('Ada Lovelace')).toBeVisible();

  // The answer comes back the same way, after it.
  await sendMessage(second.page, 'I can reproduce that.');
  await expect(messageSaying(page, 'I can reproduce that.')).toBeVisible();
  const lines = await page.locator('li[data-message]').allInnerTexts();
  expect(lines.findIndex((line) => line.includes('ten minutes'))).toBeLessThan(lines.findIndex((line) => line.includes('reproduce')));
  await second.context.close();
});

test('who is typing is shown to the other reader and is gone after a few seconds, and never to the one who types', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, TOPIC);
  await expect(composer(second.page)).toBeVisible();
  // The second reader is following the chat once the first is shown among those looking at it.
  await expect(second.page.getByRole('img', { name: 'Ada Lovelace' })).toBeVisible();
  await composer(page).click();
  await composer(page).pressSequentially('Let me write this down');
  await expect(second.page.getByText('Ada Lovelace is typing…')).toBeVisible();
  await expect(page.getByText('Ada Lovelace is typing…')).toHaveCount(0);
  await expect(second.page.getByText('Ada Lovelace is typing…')).toBeHidden({ timeout: 15_000 });
  await second.context.close();
});

test('an edit shows as edited and a delete as deleted, to the author and to the other reader', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, TOPIC);
  await sendMessage(page, 'We should retry after the redirect');
  await expect(messageSaying(second.page, 'We should retry after the redirect')).toBeVisible();

  await openMessageMenu(page, 'We should retry after the redirect');
  await page.getByRole('menuitem', { name: 'Edit' }).click();
  const editor = page.getByRole('textbox', { name: 'Edit message' });
  await editor.fill('We should retry before the redirect');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  for (const reader of [page, second.page]) {
    await expect(messageSaying(reader, 'We should retry before the redirect')).toContainText('(edited)');
    await expect(reader.getByText('We should retry after the redirect')).toHaveCount(0);
  }

  await openMessageMenu(page, 'We should retry before the redirect');
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await page.getByRole('button', { name: 'Delete message' }).click();
  for (const reader of [page, second.page]) {
    await expect(reader.getByText('Message deleted')).toBeVisible();
    await expect(reader.getByText('We should retry before the redirect')).toHaveCount(0);
  }
  await second.context.close();
});

test('a reaction shows its count to both, ringed only for whoever made it, and is taken away by a second press', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, TOPIC);
  await sendMessage(page, 'Shipping this today');
  await expect(messageSaying(second.page, 'Shipping this today')).toBeVisible();

  const message = messageSaying(second.page, 'Shipping this today');
  await message.hover();
  await message.getByRole('button', { name: 'React' }).click();
  await second.page.getByRole('searchbox', { name: 'Search emoji' }).fill('thumbs');
  await second.page.getByRole('button', { name: 'thumbs up like' }).click();

  const mine = messageSaying(second.page, 'Shipping this today').getByRole('button', { name: /^👍 1 reaction/ });
  await expect(mine).toHaveAttribute('aria-pressed', 'true');
  const theirs = messageSaying(page, 'Shipping this today').getByRole('button', { name: /^👍 1 reaction/ });
  await expect(theirs).toHaveAttribute('aria-pressed', 'false');

  // Both press it: two reacted, and the first one's press is taken away by pressing again.
  await theirs.click();
  await expect(messageSaying(page, 'Shipping this today').getByRole('button', { name: /^👍 2 reactions/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(messageSaying(second.page, 'Shipping this today').getByRole('button', { name: /^👍 2 reactions/ })).toBeVisible();
  await messageSaying(page, 'Shipping this today').getByRole('button', { name: /^👍 2 reactions/ }).click();
  await expect(messageSaying(second.page, 'Shipping this today').getByRole('button', { name: /^👍 1 reaction/ })).toHaveAttribute('aria-pressed', 'true');
  await second.context.close();
});

test('a message is pinned for everybody, listed under Pinned, and unpinned again', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, TOPIC);
  await sendMessage(page, 'The staging box is rebuilt every night');
  await sendMessage(page, 'Something else entirely');

  await openMessageMenu(page, 'The staging box is rebuilt every night');
  await page.getByRole('menuitem', { name: 'Pin', exact: true }).click();
  await expect(messageSaying(second.page, 'The staging box is rebuilt every night')).toContainText('Pinned by Ada Lovelace');

  await second.page.getByRole('button', { name: 'Pinned messages' }).click();
  await expect(second.page.getByRole('dialog', { name: 'Pinned messages' }).getByText('The staging box is rebuilt every night')).toBeVisible();
  await expect(second.page.getByRole('dialog', { name: 'Pinned messages' }).getByText('Something else entirely')).toHaveCount(0);
  await second.page.keyboard.press('Escape');

  await openMessageMenu(page, 'The staging box is rebuilt every night');
  await page.getByRole('menuitem', { name: 'Unpin' }).click();
  await expect(messageSaying(second.page, 'The staging box is rebuilt every night')).not.toContainText('Pinned by');
  await second.context.close();
});

test('a reply shows the first line of what it answers, and pressing that goes back to it', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, TOPIC);
  await sendMessage(page, 'Which environment does it run in?');
  await expect(messageSaying(second.page, 'Which environment does it run in?')).toBeVisible();

  await openMessageMenu(second.page, 'Which environment does it run in?');
  await second.page.getByRole('menuitem', { name: 'Reply' }).click();
  await expect(second.page.getByText('Replying to Ada Lovelace')).toBeVisible();
  await composer(second.page).fill('Main, on host-2');
  await composer(second.page).press('Enter');

  const reply = messageSaying(page, 'Main, on host-2');
  await expect(reply).toBeVisible();
  const excerpt = reply.getByRole('button', { name: /Ada Lovelace Which environment does it run in\?/ });
  await expect(excerpt).toBeVisible();
  await excerpt.click();
  // The message that was answered is lit for a moment where it stands.
  await expect(messageSaying(page, 'Which environment does it run in?').first()).toHaveClass(/bg-accent-subtle/);
  await second.context.close();
});

test('unread mentions are counted in the list of chats and fall to nothing once the chat has been on screen', async ({ page }) => {
  await signIn(page);
  const made = await chats('/chat', { org: ACME.org, members: ['dora', 'ada'] });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Ada, could you look at this? @ada' });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Second thing for you @ada' });

  const messageReads: string[] = [];
  page.on('request', (request) => {
    if (/\/chats\/[^/]+\/messages$/.test(new URL(request.url()).pathname)) messageReads.push(request.url());
  });
  await page.goto(MESSAGES);
  const row = page.getByRole('list', { name: 'Chats' }).getByRole('listitem').filter({ hasText: 'Dora Developer' });
  await expect(row).toBeVisible();
  await expect(row.getByText('2 unread mentions')).toBeVisible();
  await expect(row).toContainText('Second thing for you');
  expect(messageReads).toEqual([]);

  await row.getByRole('link', { name: 'Dora Developer' }).click();
  await expect(page).toHaveURL(/chat=/);
  await expect(messageSaying(page, 'Second thing for you')).toBeVisible();
  // The newest message counts as read two seconds after it is on screen.
  await expect(row.getByText('2 unread mentions')).toBeHidden({ timeout: 20_000 });
  await expect(row.getByText('Unread messages')).toBeHidden();
  // A message that arrives while the chat is open is read in its turn and leaves nothing behind.
  await chats('/say', { chat: made.chat, as: 'dora', text: 'One more, while you are here' });
  await expect(messageSaying(page, 'One more, while you are here')).toBeVisible();
  await expect(row.getByText('Unread messages')).toBeHidden({ timeout: 20_000 });
});

test('a direct chat is made from New message, opens empty, and shows up for the other person with its first line', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(MESSAGES);
  await expect(page.getByText('Talk to a teammate directly.')).toBeVisible();
  const second = await secondReader(browser, baseURL, MESSAGES);

  await page.getByRole('button', { name: 'New message' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'New message' });
  await dialog.getByRole('button', { name: /Dora Developer/ }).click();
  await dialog.getByRole('button', { name: 'Message Dora Developer' }).click();
  await expect(page).toHaveURL(/chat=/);
  await expect(page.getByText('No messages yet. Say hello to Dora Developer.')).toBeVisible();

  await sendMessage(page, 'Have you got a minute?');
  const row = second.page.getByRole('list', { name: 'Chats' }).getByRole('listitem').filter({ hasText: 'Ada Lovelace' });
  await expect(row).toBeVisible();
  await expect(row.getByText('Unread messages')).toBeVisible();
  await expect(row).toContainText('Have you got a minute?');

  // Making it again leads to the same chat: a pair of people has one direct chat in an org.
  await page.getByRole('button', { name: 'New message' }).first().click();
  await page.getByRole('dialog', { name: 'New message' }).getByRole('button', { name: /Dora Developer/ }).click();
  await page.getByRole('dialog', { name: 'New message' }).getByRole('button', { name: 'Message Dora Developer' }).click();
  await expect(messageSaying(page, 'Have you got a minute?')).toBeVisible();
  await expect(page.getByRole('list', { name: 'Chats' }).getByRole('listitem')).toHaveCount(1);
  await second.context.close();
});

test('several people make a group with a name; an admin who leaves reads on, read-only, and a developer who leaves reads no more', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto(MESSAGES);
  await page.getByRole('button', { name: 'New message' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'New message' });
  await dialog.getByRole('button', { name: /Dora Developer/ }).click();
  await dialog.getByRole('button', { name: /Linus Pauling/ }).click();
  await dialog.getByRole('textbox', { name: /^Name/ }).fill('Release crew');
  await dialog.getByRole('button', { name: 'Start a group of 2' }).click();

  await expect(page.getByRole('heading', { name: 'Release crew' })).toBeVisible();
  await expect(page.getByText('Ada Lovelace made this group')).toBeVisible();
  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await page.getByRole('menuitem', { name: 'People in this group' }).click();
  const people = page.getByRole('dialog', { name: 'People in this group' });
  await expect(people.getByText('Dora Developer')).toBeVisible();
  await expect(people.getByText('Linus Pauling')).toBeVisible();
  await page.keyboard.press('Escape');

  // An admin of the org who leaves the group is no longer in it, and may still read it as an admin does.
  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await page.keyboard.press('Escape');
  await asAdmin(page, true);
  await page.getByRole('button', { name: 'Group', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Leave group' }).click();
  await page.getByRole('button', { name: 'Leave group' }).last().click();
  await expect(page.getByRole('img', { name: 'You see this as an admin of Acme' })).toBeVisible();
  await expect(page.getByText('Ada Lovelace left')).toBeVisible();
  await expect(page.getByText('Nobody has written here yet.')).toBeVisible();
  await expect(composer(page)).toHaveCount(0);

  // A developer who leaves it reads it no more, and the chat is gone from the list.
  const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Docs crew' });
  const second = await secondReader(browser, baseURL, `${MESSAGES}?chat=${String(made.chat)}`);
  await expect(second.page.getByRole('heading', { name: 'Docs crew' })).toBeVisible();
  await second.page.getByRole('button', { name: 'Group', exact: true }).click();
  await second.page.getByRole('menuitem', { name: 'Leave group' }).click();
  await second.page.getByRole('button', { name: 'Leave group' }).last().click();
  await expect(second.page.getByText('You can no longer read this chat')).toBeVisible();
  // The group she left is gone from her list; the one she is still in stays.
  const mine = second.page.getByRole('list', { name: 'Chats' });
  await expect(mine.getByText('Release crew')).toBeVisible();
  await expect(mine.getByText('Docs crew')).toHaveCount(0);
  await second.context.close();
});

test('a guest of a group sees that chat and nothing else of the org', async ({ page }) => {
  await chats('/guest', { username: 'gus' });
  const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Launch crew', guests: ['gus'] });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Welcome aboard, Gus' });
  await signIn(page, { login: 'gus', password: 'a guest has a long password' });

  await page.goto(`${MESSAGES}?chat=${String(made.chat)}`);
  await expect(page.getByRole('heading', { name: 'Launch crew' })).toBeVisible();
  await expect(messageSaying(page, 'Welcome aboard, Gus')).toBeVisible();
  await sendMessage(page, 'Glad to be here');
  // The org itself is not theirs to read: nothing of its tree is on the screen, and no way to make chats in it.
  await expect(page.getByText('Fix login')).toHaveCount(0);
  await expect(page.getByText('Upgrade deps')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New message' })).toHaveCount(0);
  await expect(page.getByLabel('Show all chats')).toHaveCount(0);
});

test('messages are picked, with a range by Shift, and forwarded to a group chat and to a person in one act', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await chats('/seed');
  const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Launch crew' });
  await page.goto(TOPIC);
  const second = await secondReader(browser, baseURL, `${MESSAGES}?chat=${String(made.chat)}`);

  await openMessageMenu(page, 'Please take a look at');
  await page.getByRole('menuitem', { name: 'Select' }).click();
  await expect(page.getByRole('toolbar', { name: 'Selected messages' })).toContainText('1 selected');
  await page.keyboard.down('Shift');
  await page.getByRole('checkbox', { name: /Select the message of Ada Lovelace/ }).last().click();
  await page.keyboard.up('Shift');
  await expect(page.getByRole('toolbar', { name: 'Selected messages' })).toContainText('selected');
  // Let go of all but two: the range was one message, then this one.
  const picked = await page.getByRole('checkbox', { checked: true }).count();
  expect(picked).toBeGreaterThanOrEqual(2);

  await page.getByRole('button', { name: 'Forward…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Forward' });
  await dialog.getByRole('searchbox', { name: 'Search chats and people' }).fill('Launch');
  await dialog.getByRole('button', { name: /Launch crew/ }).click();
  await dialog.getByRole('searchbox', { name: 'Search chats and people' }).fill('linus');
  await dialog.getByRole('button', { name: /Linus Pauling/ }).click();
  await expect(dialog.getByText(/1 chat and Linus Pauling will get a copy of/)).toBeVisible();
  await dialog.getByRole('textbox', { name: /^Add a comment/ }).fill('For the record');
  await dialog.getByRole('button', { name: 'Forward to 2 places' }).click();
  await expect(page.getByRole('region', { name: /^Notifications/ }).getByText('Forwarded to 2 places')).toBeVisible();
  await expect(page.getByRole('toolbar', { name: 'Selected messages' })).toHaveCount(0);

  // The group's other member finds one message with the comment and a block for what was forwarded.
  const arrived = messageSaying(second.page, 'For the record');
  await expect(arrived).toBeVisible();
  await expect(arrived).toContainText('Forwarded from');
  await expect(arrived).toContainText('Fix login · topic chat');
  await expect(arrived).toContainText('Please take a look at');
  await second.context.close();
});

test('one destination that refuses stops the whole forward, and the dialog says which and why', async ({ page }) => {
  await signIn(page);
  await chats('/seed');
  const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Launch crew' });
  await chats('/switch', { node: 'Upgrade deps', off: true });
  await page.goto(TOPIC);

  await openMessageMenu(page, 'Please take a look at');
  await page.getByRole('menuitem', { name: 'Forward…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Forward' });
  await dialog.getByRole('searchbox', { name: 'Search chats and people' }).fill('Launch');
  await dialog.getByRole('button', { name: /Launch crew/ }).click();
  await dialog.getByRole('searchbox', { name: 'Search chats and people' }).fill('Upgrade');
  await dialog.getByRole('button', { name: /Upgrade deps/ }).click();
  await dialog.getByRole('button', { name: 'Forward to 2 places' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Nothing was sent');
  await expect(dialog.getByRole('alert')).toContainText('Upgrade deps');

  // Nothing went anywhere: the group is as empty as it was.
  await page.keyboard.press('Escape');
  await page.goto(`${MESSAGES}?chat=${String(made.chat)}`);
  await expect(page.getByText('No messages yet. Say something to the group.')).toBeVisible();
});

test('a timeline item quoted into a thread’s chat is a card that leads back to the item', async ({ page }) => {
  await signIn(page);
  const item = await chats('/item', { thread: ACME.mainThread, text: 'The redirect loses the next parameter.', kind: 'agent_message' });
  await chats('/say', {
    node: 'Main thread',
    as: 'dora',
    parts: [
      { type: 'text', text: 'This is the step I mean' },
      { type: 'item_ref', thread_id: ACME.mainThread, item_id: item.id },
    ],
  });
  // The left bar follows the org this device remembers, so that the thread's place is known when the thread opens.
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Chat');
  const quote = page.getByRole('link', { name: 'Go to the quoted item in the conversation' });
  await expect(quote).toContainText('Agent message');
  await expect(quote).toContainText('The redirect loses the next parameter.');
  await quote.click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}\\?item=${String(item.id)}`));
});

test('an item of the conversation is brought to the thread’s chat from its menu, stands in the box as a quote, and is sent when the person sends it', async ({ page }) => {
  await signIn(page);
  await playScene(ACME.mainThread, 'history');
  // The left bar follows the org this device remembers, so that the thread's place is known when the thread opens.
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  const question = 'Why does the login page send people to the wrong place after they sign in?';
  const item = page.locator('[data-item="u1"]');
  await expect(item).toContainText(question);
  await item.hover();
  await item.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Discuss in chat' }).click();

  // The thread's chat comes to the front with the item standing above its box; nothing has been sent.
  const bar = page.getByRole('region', { name: 'Right bar' });
  const box = bar.getByRole('textbox', { name: /^Message / });
  await expect(box).toBeVisible();
  await expect(bar.getByText(question)).toBeVisible();
  await expect(messageSaying(page, question)).toHaveCount(0);

  // Sent with a few words of the person's own, it is a message that quotes the item and leads back to it.
  await box.fill('Is this the line that matters?');
  await box.press('Enter');
  const sent = messageSaying(page, 'Is this the line that matters?');
  await expect(sent).toBeVisible();
  const quote = sent.getByRole('link', { name: 'Go to the quoted item in the conversation' });
  await expect(quote).toContainText(question);
  await quote.click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}\\?item=u1`));
});

test('items picked in the conversation are forwarded to the thread’s chat in one act, as copies that say where they came from', async ({ page }) => {
  await signIn(page);
  await playScene(ACME.mainThread, 'history');
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  const question = 'Why does the login page send people to the wrong place after they sign in?';
  const first = page.locator('[data-item="u1"]');
  await expect(first).toContainText(question);
  await first.hover();
  await first.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Select', exact: true }).click();
  await page.locator('[data-item="a1"]').getByRole('checkbox', { name: 'Select this item' }).check();

  // The thread's own chat is offered first, where the items stay live quotes, and the dialog says what the choice means.
  const bar = page.getByRole('toolbar', { name: 'Selected items' });
  await bar.getByRole('button', { name: 'Forward…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Forward' });
  await expect(dialog.getByRole('button', { name: /Main thread · thread chat/ })).toBeVisible();
  await dialog.getByRole('button', { name: /Main thread · thread chat/ }).click();
  await dialog.getByRole('textbox', { name: /^Add a comment/ }).fill('For the others to read');
  await dialog.getByRole('button', { name: 'Forward to 1 place' }).click();
  await expect(page.getByRole('region', { name: /^Notifications/ }).getByText('Forwarded to 1 place')).toBeVisible();
  await expect(bar).toHaveCount(0);

  // In the chat the copies stand in one message, under the words of whoever forwarded them.
  await showWorkbenchView(page, 'Chat');
  const sent = messageSaying(page, 'For the others to read');
  await expect(sent).toBeVisible();
  await expect(sent).toContainText(question);
  await expect(sent).toContainText('Forwarded from');
});

test('a reader who may not read the timeline is told that an item was quoted, and is shown none of it', async ({ page }) => {
  await signIn(page, PEOPLE.dora);
  // The thread and its chat are open; then the right to read threads is taken away without the page being told (what a page that was
  // opened before would still believe), and an item is quoted: the card asks for the item, and is refused. A reader who never had the
  // right has no thread to open beside its chat.
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  const bar = page.getByRole('region', { name: 'Right bar' });
  await showWorkbenchView(page, 'Chat');
  await expect(bar.getByRole('textbox', { name: /^Message / })).toBeVisible();
  const item = await chats('/item', { thread: ACME.mainThread, text: 'A secret step', kind: 'agent_message' });
  await chats('/no-timeline', { as: 'dora' });
  await chats('/say', {
    node: 'Main thread',
    as: 'ada',
    parts: [
      { type: 'text', text: 'Look at this one' },
      { type: 'item_ref', thread_id: ACME.mainThread, item_id: item.id },
    ],
  });
  await expect(messageSaying(page, 'Look at this one')).toContainText('An item of the thread');
  await expect(bar.getByText('A secret step')).toHaveCount(0);
});

test('an admin reads a chat among others without being in it: read-only, with the sentence that says so, and unseen by the others', async ({ page, browser, baseURL }) => {
  await signIn(page);
  const made = await chats('/chat', { org: ACME.org, members: ['dora', 'linus'], name: 'Secret plans' });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Do not tell Ada yet' });

  await page.goto(MESSAGES);
  await expect(page.getByRole('list', { name: 'Chats' })).toHaveCount(0);
  await expect(page.getByRole('switch', { name: /Show all chats/ })).toHaveCount(0);
  await asAdmin(page, true);
  await page.getByText("Other people's chats", { exact: true }).click();
  await page.getByRole('link', { name: 'Secret plans' }).click();

  await expect(messageSaying(page, 'Do not tell Ada yet')).toBeVisible();
  await expect(page.getByRole('img', { name: /You see this as an admin of/ })).toBeVisible();
  await expect(composer(page)).toHaveCount(0);

  // Dora has the chat open: she is told of nobody looking over it.
  const second = await secondReader(browser, baseURL, `${MESSAGES}?chat=${String(made.chat)}`);
  await expect(messageSaying(second.page, 'Do not tell Ada yet')).toBeVisible();
  await expect(second.page.getByRole('img', { name: /Ada Lovelace/ })).toHaveCount(0);
  await second.context.close();
});
