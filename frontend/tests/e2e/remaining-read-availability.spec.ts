/** Native message and port entrances keep current refusals and failed reads visible. */
import { expect, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { deny, DOUBLE, PEOPLE, scenario, signIn } from './support';

async function enter(page: Page, locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
}
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`member-message availability ${locale}/${theme}`, async ({ page }, info) => {
    await enter(page, locale, theme);
    await deny(PEOPLE.ada.login, ['members.manage']);
    const words = locale === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/org/acme/settings/policies');
      await page.getByRole('searchbox', { name: words.settings.editor.search, exact: true }).fill('org.terms');
      const tell = page.getByRole('button', { name: words.adminMessages.tellMembers, exact: true });
      await expect(tell).toHaveAttribute('aria-description', words.implicitAvailability.membersManage);
      await expect(tell).toHaveAttribute('aria-disabled', 'true');
      await tell.focus(); await page.keyboard.press('Enter');
      await expect(page.getByRole('dialog', { name: words.adminMessages.messageMembers, exact: true })).toHaveCount(0);
      await info.attach(`message-availability-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });

  test(`pending port read retry ${locale}/${theme}`, async ({ page }, info) => {
    await enter(page, locale, theme);
    const set = await fetch(`${DOUBLE}/__double/ports/set`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scene: 'full' }) });
    expect(set.ok).toBe(true);
    const words = locale === 'en' ? english : chinese;
    let failing = true;
    await page.route('**/api/v1/approvals?*', async (route) => {
      const url = new URL(route.request().url());
      if (url.searchParams.get('kind') !== 'port' || !failing) { await route.continue(); return; }
      await route.abort('failed');
    });
    for (const width of [1280, 400]) {
      failing = true;
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/resources/environments/${RUNS_ON.sandbox}?tab=ports`);
      const panel = page.getByTestId('ports-panel');
      const failure = panel.getByRole('alert').filter({ has: page.getByRole('button', { name: words.common.tryAgain, exact: true }) });
      await expect(failure).toBeVisible();
      await expect(panel.getByText(words.implicitAvailability.noPortRequests, { exact: true })).toHaveCount(0);
      failing = false;
      await failure.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      await expect(failure).toHaveCount(0);
      await info.attach(`ports-read-availability-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
