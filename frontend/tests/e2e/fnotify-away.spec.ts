/** Away status, safe failures, explicit devices/pause and grouped read/Undo across browser contexts. */
import { expect, type Page, test } from '@playwright/test';

import type { AwayPrefs, AwayRoute } from '../../src/features/settings/personal/away/types.ts';
import { approvalsMade, askCapability, notify, notifyMany, onlyChannels, settleAs } from './inbox-support.ts';
import { ACME, expectAccessibleSettled, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
interface Held { routes: AwayRoute[]; prefs: AwayPrefs; requests: { path: string; key: string | null; body: Record<string, unknown> }[]; rows: { id: string; title: string; read_at: string | null; waiting_for_me: boolean }[] }
async function away<T = unknown>(path: string, body: unknown = {}): Promise<T> {
  const reply = await fetch(`${DOUBLE}/__double/away${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!reply.ok) throw new Error(`away control ${path}: ${reply.status}`);
  return (reply.status === 204 ? null : await reply.json()) as T;
}
const held = () => away<Held>('/state');
const card = (page: Page, kind: string) => page.locator(`[data-channel][data-kind="${kind}"]`);
const groups = (page: Page) => page.locator('[data-place-group]');
async function editor(page: Page) { await page.goto('/settings/notifications'); await expect(page.getByRole('heading', { name: 'Notifications', level: 1 })).toBeVisible(); }
async function placeInbox(page: Page) {
  await page.goto('/inbox');
  await page.getByRole('combobox', { name: 'Group notifications' }).click();
  await page.getByRole('option', { name: 'By place', exact: true }).click();
}
async function bell(page: Page) { await page.getByRole('button', { name: 'Notifications for this thread' }).click(); }
async function chooseTime(page: Page, time = '12:30') {
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Date', { exact: true }).fill('2099-01-01');
  await dialog.getByLabel('Time', { exact: true }).fill(time);
}

/** A browser API double exercises permission/registration choices; it is no real push-delivery proof. */
async function pushBrowser(page: Page, permission: 'default' | 'granted' = 'default', registered = false) {
  await page.addInitScript(({ permission: initial, registered: hasLocal }) => {
    let permission = initial;
    const endpoint = 'https://push.example.test/own-browser';
    Object.assign(window, { __awayAsked: 0 });
    const subscription = { endpoint, getKey: () => new Uint8Array([1, 2, 3]).buffer, unsubscribe: () => { local = null; return Promise.resolve(true); } };
    let local: typeof subscription | null = hasLocal ? subscription : null;
    Object.defineProperty(Notification, 'permission', { configurable: true, get: () => permission });
    Object.defineProperty(Notification, 'requestPermission', { configurable: true, value: () => { const count = (window as unknown as { __awayAsked: number }).__awayAsked; Object.assign(window, { __awayAsked: count + 1 }); permission = 'granted'; return Promise.resolve('granted'); } });
    const registration = { pushManager: { getSubscription: () => Promise.resolve(local), subscribe: () => { local = subscription; return Promise.resolve(subscription); } } };
    Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: { getRegistration: () => Promise.resolve(registration), register: () => Promise.resolve(registration), ready: Promise.resolve(registration) } });
  }, { permission, registered });
}

test.beforeEach(async () => { await scenario(); });

test('the scoped bell is checked, retains stale facts, and subscription changes opt in to no outside way', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await bell(page);
  await expect(page.getByText('Outside Orch: off', { exact: true })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Edit outside notifications' })).toHaveCount(1);
  await expect(page.getByText('Configured ways do not guarantee receipt.', { exact: false })).toBeVisible();
  await page.getByRole('menuitemradio', { name: 'Off', exact: true }).click();
  expect((await held()).routes.every((route) => !route.enabled)).toBe(true);
  await bell(page);
  await away('/fault', { path: '/notifications/summary', status: 503 });
  await away('/prefs', { prefs: { approvals_always: false } });
  await expect(page.getByText('Last checked value · stale')).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Retry' })).toBeVisible();
  await away('/fault-clear');
  await page.getByRole('menuitem', { name: 'Retry' }).click();
  await bell(page);
  await expect(page.getByText('Outside Orch: off', { exact: true })).toBeVisible();
});

test('the scoped bell names configured ways, pause and attention, and opens the single editor', async ({ page }) => {
  await signIn(page);
  await editor(page);
  await card(page, 'email').getByRole('switch').click();
  await page.goto(`/thread/${ACME.mainThread}`);
  await bell(page);
  await expect(page.getByText('Outside Orch: System mail')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Pause until…' }).click();
  await chooseTime(page);
  await page.getByRole('button', { name: 'Pause notifications' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await bell(page);
  await expect(page.getByText('Outside Orch: paused until', { exact: false })).toBeVisible();
  await page.keyboard.press('Escape');
  await away('/prefs', { prefs: { pause_until: null } });
  await away('/route', { kind: 'email', route: { health: 'failed', enabled: true, failure: { code: 'acknowledgment_unknown', message: 'The mail service did not confirm its answer.', affected_devices: [] } } });
  await bell(page);
  await expect(page.getByText('Needs attention', { exact: true })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Edit outside notifications' }).click();
  await expect(page).toHaveURL(/settings\/notifications/);
  await expect(card(page, 'email').getByText('Delivery could not be confirmed through System mail')).toBeVisible();
});

test('terminal unconfirmed delivery leaves one Inbox notice, last accepted, current waiting and explicit retry', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'Work still waiting', node: ACME.mainThread });
  const failure = { kind: 'email', route: { enabled: true, health: 'failed', last_ok_at: '2026-10-13T00:00:00.000Z', failure: { code: 'acknowledgment_unknown', message: 'The service accepted a send but its reply was lost.', affected_devices: [] } } };
  await away('/route', failure);
  await away('/route', failure);
  await signIn(page);
  await page.goto('/inbox');
  await expect(page.locator('[data-kind="notification.route_attention"]')).toHaveCount(1);
  await expect(page.getByText('Nothing went out')).toHaveCount(0);
  await page.getByRole('link', { name: /Delivery could not be confirmed through System mail/ }).click();
  const mail = card(page, 'email');
  await expect(mail.getByText('Last accepted:', { exact: false })).toBeVisible();
  await mail.getByRole('button', { name: 'Retry waiting' }).click();
  await expect.poll(async () => (await held()).requests.filter((request) => request.body.retry_failed === true).length).toBe(1);
  await away('/lost-test');
  await mail.getByRole('button', { name: 'Send a test' }).click();
  await expect(mail.getByText('Test delivery could not be confirmed.')).toBeVisible();
  await expect(mail.getByText('Delivery could not be confirmed through System mail')).toBeVisible();
});

test('a removed channel keeps a safe card and removal opts in to no replacement', async ({ page }) => {
  await away('/route', { kind: 'webhook', route: { enabled: true, health: 'channel_removed', last_ok_at: '2026-10-13T00:00:00.000Z', failure: { code: 'channel_removed', message: 'This channel was removed.', affected_devices: [] } } });
  await signIn(page);
  await editor(page);
  const retained = page.locator('[data-retained-route]');
  await expect(retained.getByText('Removed: Ops room')).toBeVisible();
  await expect(retained.getByRole('switch')).toHaveCount(0);
  await retained.getByRole('button', { name: 'Choose another way' }).click();
  expect((await held()).routes.filter((route) => route.health !== 'channel_removed').every((route) => !route.enabled)).toBe(true);
  await retained.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(retained).toHaveCount(0);
});

test('this device reads real permission/context features without a prompt, and another registration is distinct', async ({ page }) => {
  await scenario({ meta: { push_public_key: 'BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U' } });
  await pushBrowser(page, 'granted', true);
  await away('/device', { endpoint: 'https://push.example.test/other', label: 'Other laptop' });
  await signIn(page);
  await editor(page);
  await expect(card(page, 'push').getByText('This device is not registered')).toBeVisible();
  await expect(card(page, 'push').getByText('Other laptop')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __awayAsked: number }).__awayAsked)).toBe(0);
  await card(page, 'push').getByRole('button', { name: 'Enable this device' }).click();
  await expect(card(page, 'push').getByText('Registered on this device')).toBeVisible();
  await expect(card(page, 'push').getByRole('switch')).not.toBeChecked();
  await card(page, 'push').getByRole('button', { name: 'Send a test' }).click();
  await expect(card(page, 'push').getByText('Test accepted by the notifier. This does not prove display on a device.')).toBeVisible();
  await card(page, 'push').getByRole('button', { name: 'Remove this device' }).click();
  await expect(card(page, 'push').getByText('This device is not registered')).toBeVisible();
  await expect(card(page, 'push').getByText('Other laptop')).toBeVisible();
});

test('blocked and unavailable push have fitting guides and no repeated permission prompt', async ({ page, browser }) => {
  await page.addInitScript(() => Object.defineProperty(Notification, 'permission', { configurable: true, get: () => 'denied' }));
  await signIn(page);
  await editor(page);
  await expect(card(page, 'push').getByText('Notifications are blocked for this website')).toBeVisible();
  await expect(card(page, 'push').getByRole('link', { name: 'Open settings instructions' })).toBeVisible();
  await expect(card(page, 'push').getByRole('button', { name: 'Enable this device' })).toHaveCount(0);
  const context = await browser.newContext();
  try {
    const other = await context.newPage();
    await other.addInitScript(() => Reflect.deleteProperty(window, 'PushManager'));
    await signIn(other);
    await editor(other);
    await expect(card(other, 'push').getByText('Push is unavailable in this browser')).toBeVisible();
    await expect(card(other, 'push').getByRole('link', { name: 'Use a supported browser/app' })).toBeVisible();
  } finally { await context.close(); }
});

test('pause remains confirmed pending, a stale choice keeps edits, and Resume changes no quiet window or switch', async ({ page, browser }) => {
  await signIn(page);
  await away('/prefs', { prefs: { quiet_hours: { start_minute: 1320, end_minute: 420 } } });
  await editor(page);
  const context = await browser.newContext();
  try {
    const other = await context.newPage();
    await signIn(other);
    await other.goto('/inbox');
    await page.getByRole('button', { name: 'Pause until…', exact: true }).click();
    await chooseTime(page);
    await away('/prefs', { prefs: { pause_until: '2099-01-01T14:00:00.000Z' } });
    await page.getByRole('button', { name: 'Pause notifications' }).click();
    await expect(page.getByText('Your choices changed in another window')).toBeVisible();
    await expect(page.getByRole('dialog').getByLabel('Time', { exact: true })).toHaveValue('12:30');
    await page.getByRole('button', { name: 'Keep my proposed change' }).click();
    await away('/fault', { path: '/me/notification-prefs', delay: 800, once: true });
    await page.getByRole('button', { name: 'Pause notifications' }).click();
    await expect(page.getByRole('button', { name: 'Pause notifications' })).toHaveAttribute('aria-busy', 'true');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(other.getByText('Paused until', { exact: false })).toBeVisible();
    await other.getByRole('button', { name: 'Resume', exact: true }).click();
    await other.getByRole('dialog').getByRole('button', { name: 'Resume', exact: true }).click();
    await expect(other.getByRole('dialog')).toHaveCount(0);
    const state = await held();
    expect(state.prefs.pause_until).toBeNull();
    expect(state.prefs.quiet_hours).toEqual({ start_minute: 1320, end_minute: 420 });
    expect(state.routes.every((route) => !route.enabled)).toBe(true);
  } finally { await context.close(); }
});

test('place groups use supplied actionability, filtered counts and no answer excerpts, and retain the chosen view', async ({ page }) => {
  const approval = await askCapability({ thread_id: ACME.mainThread, deciders: ['ada'] });
  await notify({ kind: 'capability.requested', title: 'Decision in Web', node: ACME.mainThread, approval_id: approval.id, body: 'Do not use this as an answer excerpt', at: '2026-10-13T00:00:00Z' });
  await notify({ kind: 'turn.completed', title: 'Newer work elsewhere', node: ACME.idleThread, at: '2026-10-14T00:00:00Z' });
  await signIn(page);
  await placeInbox(page);
  await expect(groups(page).first().getByRole('heading', { name: 'Web' })).toBeVisible();
  await expect(groups(page).first().getByText('Waiting for you', { exact: true })).toHaveCount(2);
  await expect(page.getByText('Do not use this as an answer excerpt')).toHaveCount(0);
  await settleAs(approval.id, 'ada');
  await expect(groups(page).getByText('Waiting for you', { exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Group notifications' })).toContainText('By place');
  await away('/hide', { ids: [ACME.mainThread] });
  await expect(groups(page).getByRole('heading', { name: 'Web' })).toHaveCount(0);
  await expect(page.getByText('Decision in Web', { exact: true })).toHaveCount(0);
});

test('group read captures one cut/key and Undo preserves new arrivals and a later read on another window', async ({ page, browser }) => {
  const first = await notify({ kind: 'turn.completed', title: 'Read again elsewhere', node: ACME.mainThread, url: '/inbox' });
  const second = await notify({ kind: 'turn.completed', title: 'Only this group read', node: ACME.mainThread, url: '/inbox' });
  const approval = await askCapability({ thread_id: ACME.mainThread, deciders: ['ada'] });
  await notify({ kind: 'capability.requested', title: 'Decision stays answered', node: ACME.mainThread, approval_id: approval.id });
  await signIn(page);
  await placeInbox(page);
  await away('/fault', { path: '/notifications/read', delay: 800, once: true });
  await groups(page).getByRole('button', { name: 'Mark this group read' }).click();
  await notify({ kind: 'turn.completed', title: 'Arrival after the cut', node: ACME.mainThread });
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeVisible();
  await settleAs(approval.id, 'ada');
  const context = await browser.newContext();
  try {
    const other = await context.newPage();
    await signIn(other);
    await other.goto('/inbox');
    await other.getByRole('link', { name: /Read again elsewhere/ }).click();
    await expect.poll(async () => (await held()).requests.filter((request) => Array.isArray(request.body.ids) && request.body.ids.includes(first.id)).length).toBeGreaterThan(0);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.getByText('Read marks undone')).toBeVisible();
    const rows = (await held()).rows;
    expect(rows.find((row) => row.id === first.id)?.read_at).not.toBeNull();
    expect(rows.find((row) => row.id === second.id)?.read_at).toBeNull();
    expect(rows.find((row) => row.title === 'Arrival after the cut')?.read_at).toBeNull();
    expect((await approvalsMade()).find((one) => one.id === approval.id)?.state).toBe('approved');
    expect(rows.find((row) => row.title === 'Decision stays answered')?.waiting_for_me).toBe(false);
  } finally { await context.close(); }
});

test('a manager sees only enabled-route counts and removal reaches each member’s retained card', async ({ page, browser }) => {
  await signIn(page);
  await editor(page);
  await card(page, 'webhook').getByRole('switch').click();
  const route = (await held()).routes.find((one) => one.channel?.kind === 'webhook');
  expect(route?.channel_id).not.toBeNull();
  const context = await browser.newContext();
  try {
    const other = await context.newPage();
    await signIn(other, PEOPLE.dora);
    await editor(other);
    await card(other, 'webhook').getByRole('switch').click();
    await page.goto('/org/acme/settings/channels');
    const row = page.locator(`li[data-channel="${route?.channel_id ?? ''}"]`);
    await expect(row.getByText('2 members switched this on')).toBeVisible();
    await expect(row.getByText('dora@example.org')).toHaveCount(0);
    await row.getByRole('button', { name: /^Remove / }).click();
    const confirmation = page.getByRole('alertdialog');
    await expect(confirmation.getByText('Removing this channel disables 2 enabled routes.', { exact: false })).toBeVisible();
    await confirmation.getByRole('button', { name: /^Remove / }).click();
    await expect(other.locator('[data-retained-route]').getByText('Removed: Ops room')).toBeVisible();
  } finally { await context.close(); }
});

test('positive target rejection offers Connect and partial push names only affected devices', async ({ page }) => {
  await away('/device', { endpoint: 'https://push.example.test/phone', label: 'Phone' });
  await away('/device', { endpoint: 'https://push.example.test/laptop', label: 'Laptop' });
  await away('/route', { kind: 'telegram', route: { enabled: false, health: 'target_gone', label: null, failure: { code: 'target_gone', message: 'The bot rejected this target.', affected_devices: [] } } });
  await away('/route', { kind: 'push', route: { enabled: true, health: 'partial', failure: { code: 'acknowledgment_unknown', message: 'The reply for one device was not confirmed.', affected_devices: ['Phone'] } } });
  await signIn(page);
  await editor(page);
  await expect(card(page, 'telegram').getByText('Switched off: The bot rejected this target.')).toBeVisible();
  await expect(card(page, 'telegram').getByRole('button', { name: 'Connect Telegram…' })).toBeVisible();
  await expect(card(page, 'push').getByText('Delivery could not be confirmed on Phone')).toBeVisible();
  await expect(card(page, 'push').getByText('Delivery could not be confirmed on Laptop')).toHaveCount(0);
  await expect(card(page, 'email').getByRole('switch')).not.toBeChecked();
});

test('a lost group reply retries the same key and an expired Undo is said explicitly', async ({ page }) => {
  await notify({ kind: 'turn.completed', title: 'One batch', node: ACME.mainThread });
  await signIn(page);
  await placeInbox(page);
  await away('/lost-read');
  await groups(page).getByRole('button', { name: 'Mark this group read' }).click();
  await expect(page.getByText('Not confirmed', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeVisible();
  const reads = (await held()).requests.filter((request) => typeof request.body.group_key === 'string');
  expect(reads).toHaveLength(2);
  expect(reads[0]?.key).toBe(reads[1]?.key);
  expect(reads[0]?.body).toEqual(reads[1]?.body);
  await away('/undo-expire');
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByText('Its undo time ended', { exact: true })).toBeVisible();
});

test('paged groups merge one heading/count and explicit Catch up uses the existing card', async ({ page }) => {
  await notifyMany(70);
  await notify({ kind: 'turn.completed', title: 'Open the counted card', node: ACME.mainThread, thread_id: ACME.mainThread });
  await signIn(page);
  await placeInbox(page);
  await expect(groups(page).getByRole('heading', { name: 'Other', exact: true })).toHaveCount(1);
  await page.getByRole('button', { name: 'Show more' }).click();
  await expect(groups(page).getByRole('heading', { name: 'Other', exact: true })).toHaveCount(1);
  await expect(groups(page).getByText('70 unread', { exact: true })).toHaveCount(1);
  await groups(page).getByRole('link', { name: 'Catch up', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Since you were last here' })).toBeVisible();
  await expect(page.getByRole('dialog').getByText('turns completed', { exact: false })).toBeVisible();
});

test('phone keys and Chinese layouts keep explicit group acts and pass axe in both themes', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await notify({ kind: 'turn.completed', title: '手机上的工作', node: ACME.mainThread, thread_id: ACME.mainThread });
  await signIn(page);
  await placeInbox(page);
  await expectAccessibleSettled(page);
  const mark = groups(page).getByRole('button', { name: 'Mark this group read' });
  await mark.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await editor(page);
  await expectAccessibleSettled(page);
  await page.request.post(`${DOUBLE}/__double/notifications/me`, { data: { locale: 'zh-CN' } });
  await page.reload();
  await expect(page.getByRole('button', { name: '暂停至…' })).toBeVisible();
  await page.getByRole('button', { name: '暂停至…' }).click();
  await expect(page.getByRole('dialog').getByText('时区：UTC')).toBeVisible();
  await expectAccessibleSettled(page, '[role="dialog"]');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
});

test('the editor empty, loading, refused and failed states offer a current retry', async ({ page }) => {
  await onlyChannels([]);
  await signIn(page);
  await editor(page);
  await expect(page.getByText('No way out is set up for you here', { exact: false })).toBeVisible();
  await away('/fault', { path: '/notification-channels', status: 403 });
  await page.reload();
  await expect(page.getByText('Your notification choices cannot be read', { exact: false })).toBeVisible();
  await away('/fault', { path: '/notification-channels', status: 503 });
  await page.reload();
  await expect(page.getByRole('button', { name: 'Try again', exact: true }).first()).toBeVisible();
  await away('/fault-clear');
  await page.getByRole('button', { name: 'Try again', exact: true }).first().click();
  await expect(page.getByText('No way out is set up for you here', { exact: false })).toBeVisible();
});
