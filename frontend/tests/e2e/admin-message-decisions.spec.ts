/** Administrator-message state decisions use real controls in both languages, widths and themes. */
import { expect, test, type Page } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { me, userRef } from '../doubles/fixtures';
import { signInForManagement } from './admin-mode-support';
import { ACME, scenario } from './support';

const ID = '00000000-0000-4000-8000-000000000321';
const MESSAGE = {
  id: ID, title: 'One accepted administrator message', body: '## Accepted message\n\nThe **accepted** body.',
  sender: userRef(me(1, 'ada', 'Ada Lovelace')), created_at: '2026-11-08T12:00:00Z', recipient_count: 4, delivery_job_id: null,
};
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

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  for (const kind of ['org', 'system'] as const) test(`${kind} holds one pending send and opens its accepted message (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const catalogue = locale === 'en' ? en : zh, words = catalogue.adminMessages;
    const org = kind === 'org' ? { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } : null;
    const message = { ...MESSAGE, org };
    const summary = { id: message.id, org, title: message.title, sender: message.sender,
      created_at: message.created_at, recipient_count: message.recipient_count, delivery_job_id: message.delivery_job_id };
    const attempts: { key: string | undefined; body: string | null }[] = [];
    let release: () => void = () => undefined, accepted = false;
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    await page.route(/\/api\/v1\/(?:orgs\/[^/]+\/messages|admin\/messages)(?:\?.*)?$/, async (route) => {
      if (route.request().method() === 'GET') {
        await route.fulfill({ json: { items: accepted ? [summary] : [], next_cursor: null, audience_count: 8 } });
        return;
      }
      attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
      await waiting; accepted = true;
      await route.fulfill({ status: 201, json: { projection: 'receipt', message_id: ID, recipient_count: 4, delivery_job_id: null } });
    });
    await page.route(`**/api/v1/messages/${ID}`, (route) => route.fulfill({ json: message }));
    try {
      await page.goto(kind === 'org' ? '/org/acme/settings/members' : '/admin/overview');
      await page.getByRole('button', { name: kind === 'org' ? words.messageMembers : words.messageEveryone, exact: true }).click();
      const dialog = page.getByRole('dialog').last();
      await dialog.getByRole('textbox', { name: words.title, exact: true }).fill(MESSAGE.title);
      await dialog.getByRole('textbox', { name: words.body, exact: true }).fill(MESSAGE.body);
      await expect(dialog.getByText((kind === 'org' ? words.sendsToMembers : words.sendsToPeople).replace('{{number}}', '8'), { exact: true })).toBeVisible();
      if (kind === 'org') await expect(dialog.getByRole('checkbox', { name: words.showBanner, exact: true })).toHaveCount(0);
      else await expect(dialog.getByRole('checkbox', { name: words.showBanner, exact: true })).not.toBeChecked();
      const send = dialog.getByRole('button', { name: words.send, exact: true });
      await expect(send).toBeEnabled(); await send.click();
      await expect(send).toHaveAttribute('aria-busy', 'true'); await expect(send).toHaveAttribute('aria-disabled', 'true');
      await expect(send).toBeFocused();
      await expect(dialog.getByRole('textbox', { name: words.body, exact: true })).toBeDisabled();
      await expect.poll(() => attempts.length).toBe(1);
      await send.focus(); await page.keyboard.press('Enter'); await send.click();
      expect(attempts).toHaveLength(1);
      await expect(dialog.getByRole('button', { name: catalogue.admin.common.done, exact: true })).toHaveCount(0);
      await info.attach('administrator-message-pending', { body: await page.screenshot(), contentType: 'image/png' });
    } finally { release(); }
    const dialog = page.getByRole('dialog').last();
    await expect(dialog.getByText((kind === 'org' ? words.sentToMembers : words.sentToPeople).replace('{{number}}', '4'), { exact: true })).toBeVisible();
    expect(attempts).toHaveLength(1); expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/iu);
    const body: unknown = JSON.parse(attempts[0]?.body ?? 'null');
    expect(body).toMatchObject({ title: MESSAGE.title, body: MESSAGE.body }); expect(body).not.toHaveProperty('banner_until');
    await dialog.getByRole('button', { name: catalogue.admin.common.done, exact: true }).click();
    const link = page.getByRole('link', { name: MESSAGE.title, exact: true });
    await expect(link).toBeVisible(); await link.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Accepted message', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('administrator-message-after-send', { body: await page.screenshot(), contentType: 'image/png' });
  });

  test(`a temporary message read retries through its visible action (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const common = (locale === 'en' ? en : zh).common;
    let reads = 0, available = false;
    await page.route(`**/api/v1/messages/${ID}`, (route) => {
      reads += 1;
      return available ? route.fulfill({ json: { ...MESSAGE, org: null } }) : route.fulfill({ status: 503, json: {
        type: 'orch:error:server.unavailable', code: 'server.unavailable', title: 'Temporarily unavailable', status: 503, detail: '', request_id: 'temporary-message-read',
      } });
    });
    await page.goto(`/messages/${ID}`);
    const retry = page.getByRole('button', { name: common.tryAgain, exact: true });
    await expect(retry).toBeVisible(); await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toHaveCount(0);
    await info.attach('administrator-message-read-failed', { body: await page.screenshot(), contentType: 'image/png' });
    const before = reads; available = true; await retry.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toBeVisible(); expect(reads).toBe(before + 1);
    await expect(page.getByRole('heading', { name: 'Accepted message', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('administrator-message-read-recovered', { body: await page.screenshot(), contentType: 'image/png' });
  });

  test(`a refused message read exposes no content or mutation retry (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await enter(page, locale, theme, width);
    const common = (locale === 'en' ? en : zh).common;
    let reads = 0;
    await page.route(`**/api/v1/messages/${ID}`, (route) => {
      reads += 1;
      return route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', status: 403, detail: '', request_id: 'refused-message-read' } });
    });
    await page.goto(`/messages/${ID}`);
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Accepted message', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: common.tryAgain, exact: true })).toHaveCount(0); expect(reads).toBe(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('administrator-message-read-refused', { body: await page.screenshot(), contentType: 'image/png' });
  });
}
