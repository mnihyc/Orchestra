/** Every state of the Inbox, the approvals and the notification settings, asked in both themes whether anything on them is a serious barrier: the Inbox with one row of every kind and a menu open, empty and filtered; approvals of the three kinds waiting, decided, requested and in the history; the settings with every card, a dialog and the kinds list open; and the counters on a small screen. */
import { expect, type Page, test } from '@playwright/test';

import { askAccess, askCapability, botStarted, notify, settleAs } from '../e2e/inbox-support.ts';
import { ACME, expectAccessibleInBothThemes, playScene, scenario, signIn } from '../e2e/support.ts';

const MAIN = ACME.mainThread;
const REPRO = ACME.repro;

test.beforeEach(async () => {
  await scenario();
});

/** One notification of every kind the contract names, some read, over three days. */
const KINDS = [
  'turn.completed',
  'turn.failed',
  'thread.blocked',
  'approval.requested',
  'capability.requested',
  'access.requested',
  'access.decided',
  'milestone',
  'announcement',
  'mention',
  'chat.message',
  'doc.mention',
  'doc.comment',
  'provider.exhausted',
  'provider.attention',
  'machine.offline',
  'environment.idle_stop',
  'environment.update',
  'environment.attention',
  'cap.warning',
  'cap.reached',
  'job.finished',
  'invitation',
  'member.moved',
  'sign_in.attention',
] as const;

async function seedEveryKind(): Promise<void> {
  for (const [index, kind] of KINDS.entries()) {
    await notify({
      kind,
      title: `A notification of the kind ${kind}`,
      body: index % 3 === 0 ? 'What happened, in a first line that is long enough to need more than one line on a narrow screen, so that it is cut where it should be.' : 'A short line.',
      ...(index % 2 === 0 ? { thread_id: MAIN, node: MAIN } : {}),
      ...(kind === 'chat.message' ? { chat_id: '00000000-0000-4000-8009-000000000004', node: ACME.fixLogin } : {}),
      read: index % 4 === 1,
      at: new Date(Date.now() - index * 3.5 * 3_600_000).toISOString(),
    });
  }
}

async function open(page: Page, path: string): Promise<void> {
  await signIn(page);
  await page.goto(path);
}

test('the Inbox with a row of every kind, read and unread, over three days', async ({ page }) => {
  await seedEveryKind();
  await open(page, '/inbox');
  await expect(page.locator('li[data-kind]')).toHaveCount(KINDS.length);
  await expectAccessibleInBothThemes(page);
});

test('the Inbox with the menu of a row open', async ({ page }) => {
  await seedEveryKind();
  await open(page, '/inbox');
  await page.locator('li[data-kind="mention"]').getByRole('button', { name: 'Menu of this notification' }).click();
  await expect(page.getByRole('menuitem', { name: 'Mark as read' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the Inbox filtered to what is unread, to mentions, and with nothing in it', async ({ page }) => {
  await seedEveryKind();
  await open(page, '/inbox?filter=unread');
  await expect(page.locator('li[data-kind]').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('radio', { name: 'Mentions' }).click();
  await expect(page.locator('li[data-kind="mention"]')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: 'Mark all read' }).click();
  await page.getByRole('radio', { name: /^Unread/ }).click();
  await expect(page.getByText('You have read everything')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the Inbox before anything has come', async ({ page }) => {
  await open(page, '/inbox');
  await expect(page.getByText('Nothing here yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('what waits, of the three kinds, with its controls', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 4600 });
  await playScene(MAIN, 'everything');
  await askCapability({ requester: 'grace', title: 'GPU for Training box', risk: 'critical' });
  const batch = '00000000-0000-4000-8014-000000000001';
  await askAccess({
    id: '00000000-0000-4000-8012-0000000000a1',
    requester: 'dora',
    batch_id: batch,
    title: 'Access to Fix login',
    object: { kind: 'thread', id: MAIN, name: 'Main thread' },
    permissions: ['thread.view', 'thread.interact', 'terminal.manage'],
    reason: 'I would like to work with the agent on the login redirect.',
    for_thread: { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
  });
  await askAccess({
    id: '00000000-0000-4000-8012-0000000000a2',
    requester: 'dora',
    batch_id: batch,
    title: 'Access to Claude Max',
    object: { kind: 'provider', id: '00000000-0000-4000-8008-000000000001', name: 'Claude Max' },
    permissions: ['provider.use'],
    options: { models: ['claude-opus', 'claude-sonnet'], efforts: ['medium', 'high'] },
  });
  await open(page, '/approvals');
  await expect(page.getByRole('region', { name: 'GPU for Training box' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Dora Developer asks for access to 2 things' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a request decided meanwhile, a decided line opened, and the requested and history lists', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  const first = await askCapability({ requester: 'dora', title: 'GPU for Training box', reason: 'Training needs it.' });
  await askAccess({ requester: 'dora', title: 'Access to Repro', object: { kind: 'thread', id: REPRO, name: 'Repro' }, permissions: ['thread.view'] });
  await open(page, '/approvals');
  await expect(page.getByRole('region', { name: 'GPU for Training box' }).getByRole('button', { name: 'Approve', exact: true })).toBeVisible();
  await settleAs(first.id, 'grace', 'approved');
  await expect(page.getByText('Already answered by Grace Hopper')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('region', { name: 'GPU for Training box' }).getByRole('button', { name: /Approved by Grace Hopper/ }).click();
  await expect(page.getByText('Training needs it.')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('tab', { name: 'History' }).click();
  await expect(page.getByRole('region', { name: 'GPU for Training box' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

});

test('what the person asked for, waiting and answered', async ({ page }) => {
  const answered = await askAccess({ requester: 'dora', title: 'Access to Repro', object: { kind: 'thread', id: REPRO, name: 'Repro' }, permissions: ['thread.view'] });
  await askAccess({ requester: 'dora', title: 'Access to Fix login', object: { kind: 'thread', id: MAIN, name: 'Main thread' }, permissions: ['thread.view', 'thread.interact'], reason: 'To follow the redirect work.' });
  await settleAs(answered.id, 'ada', 'approved');
  await signIn(page, { login: 'dora', password: 'compilers and goto' });
  await page.goto('/approvals?tab=requested');
  await expect(page.getByRole('region', { name: 'Access to Fix login' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('nothing waits, in each tab', async ({ page }) => {
  await open(page, '/approvals');
  await expect(page.getByText('Nothing waits for you')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('tab', { name: 'Requested by me' }).click();
  await expect(page.getByText('You have asked for nothing').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('tab', { name: 'History' }).click();
  await expect(page.getByText('Nothing was decided yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the notification settings with every card, switched on and off', async ({ page }) => {
  await open(page, '/settings/notifications');
  await expect(page.getByRole('region', { name: 'Mail · this system' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  const mail = page.getByRole('region', { name: 'Mail · this system' });
  await mail.getByRole('switch', { name: 'Mail · this system' }).click();
  await expect(mail.getByRole('switch', { name: 'Mail · this system' })).toBeChecked();
  await mail.getByRole('checkbox', { name: 'Only what I have not read after' }).click();
  await mail.getByRole('radio', { name: 'Only these kinds' }).click();
  await expectAccessibleInBothThemes(page);
});

test('the kinds list of a route, open', async ({ page }) => {
  await open(page, '/settings/notifications');
  const mail = page.getByRole('region', { name: 'Mail · this system' });
  await mail.getByRole('switch', { name: 'Mail · this system' }).click();
  await mail.getByRole('radio', { name: 'Only these kinds' }).click();
  await mail.getByRole('button', { name: 'Choose kinds' }).click();
  await expect(page.getByRole('checkbox', { name: 'Approval needed' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the dialog that connects a bot, and the one that makes a webhook', async ({ page }) => {
  await open(page, '/settings/notifications');
  await page.getByRole('region', { name: "Telegram · Acme's bot" }).getByRole('button', { name: 'Connect Telegram…' }).click();
  const connect = page.getByRole('dialog', { name: 'Connect Telegram' });
  await expect(connect.getByRole('img', { name: 'QR code of the link' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await botStarted();
  await expect(page.getByRole('dialog')).toHaveCount(0);

  await page.getByRole('button', { name: 'Add a webhook…' }).click();
  const webhook = page.getByRole('dialog', { name: 'A webhook of my own' });
  await webhook.getByRole('button', { name: 'Add the webhook' }).click();
  await expect(webhook.getByText('Give it a name.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the notification settings that cannot reach the person: mail not confirmed, push refused', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(Notification, 'permission', { get: () => 'denied', configurable: true }));
  await fetch(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/notifications/mail/verified`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ user: 'ada', verified: false }),
  });
  await open(page, '/settings/notifications');
  await expect(page.getByText('Confirm ada@example.org first')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the counters and the pages on a small screen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await seedEveryKind();
  await playScene(REPRO, 'waiting');
  await open(page, '/inbox');
  await expect(page.locator('li[data-kind]').first()).toBeVisible();
  await expect(page.getByRole('link', { name: /^Approvals/ }).last()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto('/approvals');
  await expect(page.getByRole('region', { name: 'Run npm install left-pad' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto('/settings/notifications');
  await expect(page.getByRole('region', { name: 'Mail · this system' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
