/** The Inbox and its counters, against the fake API and its socket: a notification that arrives while it is open and the counters that move, read and unread, one row at a time, by place and all of them, the filters and the pages, a thread that reads its own, a frame that was lost, and a second window that learns of what the first read. */
import { expect, type Page, test } from '@playwright/test';

import { askAccess, inboxHeld, notify, notifyMany } from './inbox-support.ts';
import { ACME, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;
const REPRO = ACME.repro;

test.beforeEach(async () => {
  await scenario();
});

const rail = (page: Page) => page.getByRole('navigation', { name: 'Orgs and your own places' });
const leftBar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const rows = (page: Page) => page.locator('li[data-kind]');
const unreadRows = (page: Page) => page.locator('li[data-kind][data-unread="true"]');

test('an Inbox request names its thread and person, opens the exact old item, and a decision changes the counters without a summary GET', async ({ page }) => {
  const request = await askAccess({ requester: 'grace', title: 'Review this older request', thread_id: MAIN,
    node: { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
    object: { id: MAIN, kind: 'thread', name: 'Main thread', owner: null }, permissions: ['thread.interact'], reason: 'Please review the release.' });
  // Put the target beyond the normal approvals first page; the Inbox must not choose a different card.
  for (let index = 0; index < 21; index += 1) await askAccess({ title: `Another request ${index}` });
  await notify({ kind: 'access.requested', title: 'Old stored subject', thread_id: MAIN, node: MAIN, approval_id: request.id, url: '/approvals' });
  await signIn(page);
  await openInbox(page);
  const notice = page.locator('li[data-kind="access.requested"]');
  await expect(notice.locator('[data-inbox-title]')).toContainText('Main thread');
  await expect(notice.locator('[data-inbox-event]')).toHaveText('Grace Hopper requested access');
  const lists: string[] = [];
  page.on('request', (request) => { const url = new URL(request.url()); if (request.method() === 'GET' && url.pathname.endsWith('/api/approvals')) lists.push(url.pathname); });
  await notice.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/approvals\\?approval=${request.id}$`));
  const exact = page.locator(`[data-focused-approval="${request.id}"]`);
  await expect(exact).toContainText('Please review the release.');
  expect(lists).toEqual([]);
  const before = (await inboxHeld()).summary.approvals_pending;
  const summaries: string[] = [];
  page.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname.endsWith('/notifications/summary')) summaries.push(request.url()); });
  await exact.getByRole('button', { name: 'Approve as asked' }).click();
  await expect(exact).toContainText('Approved by Ada Lovelace');
  await expect.poll(async () => (await inboxHeld()).summary.approvals_pending).toBe(before - 1);
  await expect(rail(page).getByRole('link', { name: new RegExp(`Approvals.*${before - 1}`) })).toBeVisible();
  expect(summaries).toEqual([]);
});

test('an open Inbox replaces a request with its actual decider on the same row, without a repair read', async ({ page }) => {
  const request = await askAccess({ requester: 'grace', title: 'Review the release', thread_id: MAIN,
    node: { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
    object: { id: MAIN, kind: 'thread', name: 'Main thread', owner: null }, permissions: ['thread.interact'], reason: 'Please review the release.' });
  await notify({ kind: 'access.requested', title: 'Old stored subject', thread_id: MAIN, node: MAIN, approval_id: request.id, url: '/approvals' });
  await signIn(page); await openInbox(page);
  const notice = page.locator('li[data-kind="access.requested"]');
  await expect(notice.locator('[data-inbox-title]')).toContainText('Main thread');
  await expect(notice.locator('[data-inbox-event]')).toHaveText('Grace Hopper requested access');
  const reads: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === 'GET' && (path.endsWith('/notifications') || path.endsWith('/notifications/summary'))) reads.push(path);
  });
  const decision = await page.context().newPage();
  try {
    await decision.goto(`/approvals?approval=${request.id}`);
    const exact = decision.locator(`[data-focused-approval="${request.id}"]`);
    await expect(exact).toContainText('Please review the release.');
    await exact.getByRole('button', { name: 'Approve as asked' }).click();
    await expect(exact).toContainText('Approved by Ada Lovelace');
    await expect(notice.locator('[data-inbox-event]')).toHaveText('Allowed by Ada Lovelace');
    await expect(notice).toHaveAttribute('data-unread', 'false');
    await expect(notice).not.toContainText('Grace Hopper requested access');
    await expect(notice).not.toContainText('Waiting for you');
    await expect(notice).toHaveCount(1);
    await expect(page).toHaveURL(/\/inbox$/u);
    expect(reads).toEqual([]);
  } finally { await decision.close(); }
});

/** Opens the Inbox through the rail, as a person does, so that nothing of the page is loaded twice. */
async function openInbox(page: Page): Promise<void> {
  await rail(page).getByRole('link', { name: /^Inbox/ }).click();
  await expect(page).toHaveURL(/\/inbox/);
}

test('a notification that arrives while the Inbox is open appears at the top and moves the counters of the rail, the only place that shows them', async ({ page }) => {
  await signIn(page);
  await openInbox(page);
  await expect(page.getByText('Nothing here yet')).toBeVisible();
  // Nothing is unread: no number anywhere, and the places are named without one.
  await expect(rail(page).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();

  await notify({ kind: 'turn.failed', title: 'Main thread: turn failed', body: 'The provider answered with an error.', thread_id: MAIN, node: MAIN });
  await expect(page.locator('li[data-kind="turn.failed"] [data-inbox-title]')).toHaveText('Unread: Main thread');
  await expect(page.locator('li[data-kind="turn.failed"] [data-inbox-event]')).toHaveText('Turn failed');
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  // The left bar has no Inbox of its own: the rail is where it and its number stand.
  await expect(leftBar(page).getByRole('link', { name: /^Inbox/ })).toHaveCount(0);

  await notify({ kind: 'mention', title: 'Grace Hopper mentioned you in Fix login', body: '@ada can you look at the redirect?', thread_id: MAIN, node: MAIN });
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();
  // Newest first: the mention stands above the failure, and both are drawn as unread.
  await expect(rows(page).first()).toHaveAttribute('data-kind', 'mention');
  await expect(unreadRows(page)).toHaveCount(2);
  await expect(page.getByText('Today', { exact: true })).toBeVisible();
});

test('the counters are there on every screen and move while the person is somewhere else', async ({ page }) => {
  await signIn(page);
  await page.goto('/approvals');
  await expect(rail(page).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();
  await notify({ kind: 'milestone', title: 'Baseline run reached a milestone' });
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  await notify({ kind: 'milestone', title: 'Baseline run reached another' });
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();
});

test('opening a notification goes to what it is about and reads it, and its menu reads one without going there', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN, url: '/inbox?filter=mentions' });
  await notify({ kind: 'mention', title: 'Grace mentioned you in Repro', thread_id: REPRO, node: REPRO, url: `/thread/${REPRO}` });
  await signIn(page);
  await openInbox(page);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();

  await page.locator('li[data-kind="mention"]').getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/thread/${REPRO}`));
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(/\/inbox/);
  const completed = page.locator('li[data-kind="turn.completed"]');
  await expect(completed).toHaveAttribute('data-unread', 'true');
  await expect(page.locator('li[data-kind="mention"]')).toHaveAttribute('data-unread', 'false');

  // Its menu reads it where it stands: the person stays on the Inbox.
  await completed.getByRole('button', { name: 'Menu of this notification' }).click();
  await page.getByRole('menuitem', { name: 'Mark as read' }).click();
  await expect(completed).toHaveAttribute('data-unread', 'false');
  await expect(page).toHaveURL(/\/inbox/);
  await expect(rail(page).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();
  // The server holds what the page shows.
  expect((await inboxHeld()).summary.unread).toBe(0);
});

test('everything from one place is read together, from the menu of any of its rows', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN });
  await notify({ kind: 'turn.failed', title: 'Main thread: turn failed', thread_id: MAIN, node: MAIN });
  await notify({ kind: 'turn.completed', title: 'Repro: turn completed', thread_id: REPRO, node: REPRO });
  await signIn(page);
  await openInbox(page);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 3 unread' })).toBeVisible();

  await page.locator('li[data-kind="turn.failed"]').getByRole('button', { name: 'Menu of this notification' }).click();
  await page.getByRole('menuitem', { name: /Mark everything from .*Main thread.* read/ }).click();
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  await expect(unreadRows(page)).toHaveCount(1);
  await expect(unreadRows(page).locator('[data-inbox-title]')).toHaveText('Unread: Repro');
  await expect(unreadRows(page).locator('[data-inbox-event]')).toHaveText('Turn completed');
});

test('the rows of a chat are read together as well', async ({ page }) => {
  const chat = '00000000-0000-4000-8009-000000000004';
  await notify({ kind: 'chat.message', title: 'Fix login', body: 'Grace Hopper: the redirect is fixed', chat_id: chat, node: ACME.fixLogin, url: '/org/acme/messages' });
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN });
  await signIn(page);
  await openInbox(page);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();
  await page.locator('li[data-kind="chat.message"]').getByRole('button', { name: 'Menu of this notification' }).click();
  await page.getByRole('menuitem', { name: "Mark this chat's notifications read" }).click();
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  await expect(page.locator('li[data-kind="chat.message"]')).toHaveAttribute('data-unread', 'false');
});

test('Mark all read reads every row, those the page has not loaded included, and what the server holds says the same', async ({ page }) => {
  // An unread row three days old, far beyond the first page, under sixty newer ones that were read.
  await notify({ kind: 'sign_in.attention', title: 'Somebody tried to sign in as you', at: new Date(Date.now() - 3 * 24 * 3_600_000).toISOString() });
  await notifyMany(60, { read: true });
  await signIn(page);
  await openInbox(page);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  await page.getByRole('button', { name: 'Mark all read' }).click();
  await expect(rail(page).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Mark all read' })).toBeDisabled();
  expect((await inboxHeld()).summary.unread).toBe(0);
  await page.reload();
  await expect(rail(page).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();
});

test('Unread finds the unread rows beyond the first page, All shows older rows a page at a time, and Mentions keeps to those that name the person', async ({ page }) => {
  await notify({ kind: 'mention', title: 'An old mention', at: new Date(Date.now() - 4 * 24 * 3_600_000).toISOString() });
  await notify({ kind: 'invitation', title: 'An old invitation', read: true, at: new Date(Date.now() - 5 * 24 * 3_600_000).toISOString() });
  await notifyMany(60, { read: true });
  await signIn(page);
  await openInbox(page);

  // The first page: fifty rows, the oldest ones not among them.
  await expect(rows(page)).toHaveCount(50);
  await expect(page.getByText('An old mention')).toHaveCount(0);

  await page.getByRole('radio', { name: /^Unread/ }).click();
  await expect(page).toHaveURL(/filter=unread/);
  await expect(rows(page)).toHaveCount(1);
  await expect(page.getByText('An old mention')).toBeVisible();

  await page.getByRole('radio', { name: 'Mentions' }).click();
  // Nothing among what is loaded names the person: the page says so and offers the older ones.
  await expect(page.getByText('Nothing among what is loaded. Older notifications may hold some.')).toBeVisible();
  await page.getByRole('button', { name: 'Show more' }).click();
  await expect(page.getByText('An old mention')).toBeVisible();
  await expect(rows(page)).toHaveCount(1);

  // The pages now reach the end: All holds every row, the oldest included, and offers no more.
  await page.getByRole('radio', { name: 'All' }).click();
  await expect(page).not.toHaveURL(/filter=/);
  await expect(rows(page)).toHaveCount(62);
  await expect(page.getByText('An old invitation')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Show more' })).toHaveCount(0);
});

test('All asks for older rows a page at a time', async ({ page }) => {
  await notifyMany(70, { read: true });
  await signIn(page);
  await openInbox(page);
  await expect(rows(page)).toHaveCount(50);
  await page.getByRole('button', { name: 'Show more' }).click();
  await expect(rows(page)).toHaveCount(70);
  await expect(page.getByRole('button', { name: 'Show more' })).toHaveCount(0);
});

test('an empty Unread and an empty Mentions say what they are', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN, read: true });
  await signIn(page);
  await page.goto('/inbox?filter=unread');
  await expect(page.getByText('You have read everything')).toBeVisible();
  await page.getByRole('radio', { name: 'Mentions' }).click();
  await expect(page.getByText('Nobody has mentioned you')).toBeVisible();
});

test('opening a thread reads what the person was told about it, and what arrives while it is open', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN });
  await notify({ kind: 'turn.completed', title: 'Repro: turn completed', thread_id: REPRO, node: REPRO });
  await signIn(page);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();

  await page.goto(`/thread/${MAIN}`);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  expect((await inboxHeld()).rows.find((row) => row.thread_id === MAIN)?.read_at).not.toBeNull();
  expect((await inboxHeld()).rows.find((row) => row.thread_id === REPRO)?.read_at).toBeNull();

  // What comes about this thread while it is in front of the person is read as it comes.
  await notify({ kind: 'turn.failed', title: 'Main thread: turn failed', thread_id: MAIN, node: MAIN });
  await expect.poll(async () => (await inboxHeld()).summary.unread).toBe(1);
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
});

test('a frame that was lost is repaired by the next one: both rows are there and the counters are right', async ({ page }) => {
  await signIn(page);
  await openInbox(page);
  await expect(page.getByText('Nothing here yet')).toBeVisible();
  await notify({ kind: 'turn.completed', title: 'The row nobody was told of', thread_id: MAIN, node: MAIN, lose: true });
  await notify({ kind: 'turn.failed', title: 'The row that came after it', thread_id: REPRO, node: REPRO });
  await expect(page.getByRole('link', { name: /The row that came after it/ })).toBeVisible();
  await expect(page.getByRole('link', { name: /The row nobody was told of/ })).toBeVisible();
  await expect(rail(page).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();
});

test('a second window learns of what the first read, and a row that arrives is in both', async ({ page, context }) => {
  await notify({ kind: 'turn.completed', title: 'Main thread: turn completed', thread_id: MAIN, node: MAIN });
  await notify({ kind: 'turn.failed', title: 'Repro: turn failed', thread_id: REPRO, node: REPRO });
  await signIn(page);
  await openInbox(page);
  const other = await context.newPage();
  await other.goto('/inbox');
  await expect(rail(other).getByRole('link', { name: 'Inbox, 2 unread' })).toBeVisible();

  await page.getByRole('button', { name: 'Mark all read' }).click();
  await expect(rail(other).getByRole('link', { name: 'Inbox', exact: true })).toBeVisible();
  await expect(other.locator('li[data-kind][data-unread="true"]')).toHaveCount(0);

  await notify({ kind: 'milestone', title: 'A milestone for both windows' });
  await expect(other.getByRole('link', { name: /A milestone for both windows/ })).toBeVisible();
  await expect(page.getByRole('link', { name: /A milestone for both windows/ })).toBeVisible();
});
