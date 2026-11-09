/** Own click paths for both senders, the terms shortcut and the received Inbox message. */
import { expect, test, type Page } from '@playwright/test';
import en from '../../src/i18n/en.json';
import zh from '../../src/i18n/zh-CN.json';
import { me, userRef } from '../doubles/fixtures';
import { signInForManagement } from './admin-mode-support';
import { ACME, DOUBLE, scenario } from './support';

const ID = '00000000-0000-4000-8000-000000000321';
const SENDER = userRef(me(1, 'ada', 'Ada Lovelace'));
const MESSAGE = { id: ID, title: 'A message for everyone', body: '## What changed\n\nRead this **important** change.',
  sender: SENDER, created_at: '2026-11-08T12:00:00Z', recipient_count: 4, delivery_job_id: null };
async function enter(page: Page, locale: 'en' | 'zh-CN', theme: 'light' | 'dark', width: number) {
  await scenario();
  await page.addInitScript(({ theme }) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' })), { theme });
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme });
  await signInForManagement(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
  }
}
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) {
  for (const kind of ['org', 'system'] as const) test(`${kind} sends one retryable Markdown message in ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const words = (locale === 'en' ? en : zh).adminMessages;
    const common = (locale === 'en' ? en : zh).admin.common;
    const org = kind === 'org' ? { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } : null;
    const message = { ...MESSAGE, org };
    const attempts: { key: string | undefined; body: string | null }[] = [];
    await page.route(/\/api\/v1\/(?:orgs\/[^/]+\/messages|admin\/messages)(?:\?.*)?$/, async (route) => {
      if (route.request().method() === 'GET') {
        const summary = { id: message.id, org: message.org, title: message.title, sender: message.sender,
          created_at: message.created_at, recipient_count: message.recipient_count, delivery_job_id: message.delivery_job_id };
        await route.fulfill({ json: { items: attempts.length > 1 ? [summary] : [], next_cursor: null, audience_count: 8 } });
      } else {
        attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
        if (attempts.length === 1) await route.abort('connectionfailed');
        else await route.fulfill({ status: 201, json: { projection: 'receipt', message_id: ID, recipient_count: 4, delivery_job_id: null } });
      }
    });
    await page.route(`**/api/v1/messages/${ID}`, (route) => route.fulfill({ json: message }));
    await page.goto(kind === 'org' ? '/org/acme/settings/members' : '/admin/overview');
    await page.getByRole('button', { name: kind === 'org' ? words.messageMembers : words.messageEveryone, exact: true }).click();
    const dialog = page.getByRole('dialog').last();
    await dialog.getByRole('textbox', { name: words.title, exact: true }).fill(MESSAGE.title);
    await dialog.getByRole('textbox', { name: words.body, exact: true }).fill(MESSAGE.body);
    await dialog.getByRole('tab', { name: words.preview, exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'What changed', exact: true })).toBeVisible();
    await dialog.getByRole('tab', { name: words.write, exact: true }).click();
    if (kind === 'system') {
      await dialog.getByRole('checkbox', { name: words.showBanner, exact: true }).check();
      await dialog.getByRole('textbox', { name: words.until, exact: true }).fill('2027-01-02 12:30');
    }
    await info.attach('message-before-send', { body: await page.screenshot(), contentType: 'image/png' });
    const send = dialog.getByRole('button', { name: words.send, exact: true });
    await expect(send).toBeEnabled();
    await send.click();
    await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: words.body, exact: true })).toHaveValue(MESSAGE.body);
    await send.focus(); await page.keyboard.press('Enter');
    await expect(dialog.getByText((kind === 'org' ? words.sentToMembers : words.sentToPeople).replace('{{number}}', '4'), { exact: true })).toBeVisible();
    expect(attempts).toHaveLength(2);
    expect(attempts[0]?.key).toMatch(/^[a-f0-9-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    const sent: unknown = JSON.parse(attempts[0]?.body ?? 'null');
    expect(sent).toMatchObject({ title: MESSAGE.title, body: MESSAGE.body });
    if (kind === 'system') expect(sent).toHaveProperty('banner_until');
    await dialog.getByRole('button', { name: common.done, exact: true }).click();
    const link = page.getByRole('link', { name: MESSAGE.title, exact: true });
    await expect(link).toBeVisible();
    await link.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'What changed', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('message-read', { body: await page.screenshot(), contentType: 'image/png' });
  });

  test(`the terms field opens the same members composer in ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const catalogue = locale === 'en' ? en : zh;
    const words = catalogue.adminMessages;
    await page.route(`**/api/v1/orgs/${ACME.org}/messages**`, (route) => route.fulfill({ json: { items: [], next_cursor: null, audience_count: 8 } }));
    await page.goto('/org/acme/settings/defaults');
    await page.getByRole('searchbox', { name: catalogue.settings.editor.search, exact: true }).fill('org.terms');
    const tell = page.getByRole('button', { name: words.tellMembers, exact: true });
    await expect(tell).toBeVisible();
    await tell.focus(); await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog').last();
    await expect(dialog.getByRole('textbox', { name: words.title, exact: true })).toHaveValue(words.termsChanged.replace('{{org}}', 'Acme'));
    await expect(dialog.getByRole('textbox', { name: words.body, exact: true })).toHaveValue('');
    await info.attach('terms-message-composer', { body: await page.screenshot(), contentType: 'image/png' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });

  test(`an Inbox administrator message opens its exact reader in ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const words = (locale === 'en' ? en : zh).adminMessages;
    const notice = await page.request.post(`${DOUBLE}/__double/notifications/row`, { data: {
      kind: 'admin_message', title: MESSAGE.title, body: MESSAGE.body, actor: 'ada', node: ACME.org,
      url: `/messages/${ID}`, parameters: { message_id: ID, org_id: ACME.org, org_name: 'Acme' },
    } });
    expect(notice.ok()).toBe(true);
    await page.route(`**/api/v1/messages/${ID}`, (route) => route.fulfill({ json: { ...MESSAGE, org: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } } }));
    await page.goto('/inbox');
    const row = page.locator('li[data-kind="admin_message"]');
    await expect(row).toContainText(words.fromOrg.replace('{{org}}', 'Acme'));
    await expect(row).not.toContainText('What changed');
    const link = row.getByRole('link', { name: MESSAGE.title, exact: true });
    await link.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'What changed', exact: true })).toBeVisible();
    await expect(page.getByText(words.fromOrg.replace('{{org}}', 'Acme'), { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('inbox-message', { body: await page.screenshot(), contentType: 'image/png' });
  });
}


// Inbox > received message > native Markdown reader; literal markup stays written text.
// 收件箱 > 已收到的消息 > 共用 Markdown 阅读器；字面标记仍然只是文字。
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) test(`received message markup remains data in ${locale}/${theme}/${width}`, async ({ page }, info) => {
  await enter(page, locale, theme, width);
  const body = '## Reader formatting\n\n<aside data-message-note="literal">Written words</aside>\n\n![Diagram](https://diagram.example.invalid/example.png)\n\n[A note](note:example)';
  const row = { ...MESSAGE, body, org: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } };
  let imageRequests = 0;
  await page.route('https://diagram.example.invalid/example.png', async (route) => { imageRequests += 1; await route.abort(); });
  const notice = await page.request.post(`${DOUBLE}/__double/notifications/row`, { data: {
    kind: 'admin_message', title: row.title, body, actor: 'ada', node: ACME.org,
    url: `/messages/${ID}`, parameters: { message_id: ID, org_id: ACME.org, org_name: 'Acme' },
  } });
  expect(notice.ok()).toBe(true);
  await page.route(`**/api/v1/messages/${ID}`, (route) => route.fulfill({ json: row }));
  await page.goto('/inbox');
  const link = page.locator('li[data-kind="admin_message"]').getByRole('link', { name: row.title, exact: true });
  await link.focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Reader formatting', exact: true })).toBeVisible();
  await expect(page.getByText('<aside data-message-note="literal">Written words</aside>', { exact: true })).toBeVisible();
  await expect(page.locator('[data-message-note]')).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Diagram', exact: true })).toHaveAttribute('href', 'https://diagram.example.invalid/example.png');
  await expect(page.getByRole('img', { name: 'Diagram', exact: true })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'A note', exact: true })).toHaveCount(0);
  expect(imageRequests).toBe(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await info.attach('message-literal-markup', { body: await page.screenshot(), contentType: 'image/png' });
});
