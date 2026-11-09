/** A repeated one-time browser entry closes its empty popup and never navigates with an old code. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json';
import zh from '../../src/i18n/zh-CN.json';
import { RUNS_ON } from '../doubles/fixtures';
import { DOUBLE, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440])
for (const theme of ['light', 'dark'] as const) {
  test(`port entry renewal and deliberate new entry in ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    const configured = await fetch(`${DOUBLE}/__double/ports/set`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scene: 'full' }) });
    expect(configured.ok).toBe(true);
    await page.addInitScript(({ theme }) => {
      localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' }));
    }, { theme });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? en : zh;
    const attempts: { key: string | undefined; body: string | null }[] = [];
    await page.route('**/api/v1/environments/*/ports/*/enter', async (route) => {
      attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
      if (attempts.length === 1) await route.fulfill({ status: 401, json: { type: 'orch:error:auth.required', code: 'auth.required', status: 401, title: 'Sign in', detail: '', request_id: 'renew-port-entry' } });
      else await route.fulfill({ json: { projection: 'receipt', ticket_id: `00000000-0000-4000-8000-${String(attempts.length).padStart(12, '0')}`, ticket_expires_at: '2035-01-01T00:00:00Z' } });
    });
    await page.goto(`/resources/environments/${RUNS_ON.sandbox}?tab=ports`);
    const port = page.getByTestId('port-3000');
    const open = port.getByRole('button', { name: words.ports.open, exact: true });
    await expect(open).toBeEnabled();
    const popup = page.waitForEvent('popup');
    await open.click();
    const first = await popup;
    await expect.poll(() => first.isClosed()).toBe(true);
    await expect(port.getByText(words.ui.receipt.accepted, { exact: true })).toBeVisible();
    expect(attempts).toHaveLength(2);
    expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    const nextPopup = page.waitForEvent('popup');
    await open.focus(); await page.keyboard.press('Enter');
    const next = await nextPopup;
    await expect.poll(() => next.isClosed()).toBe(true);
    await expect.poll(() => attempts.length).toBe(3);
    expect(attempts[2]?.body).toBe(attempts[0]?.body);
    expect(attempts[2]?.key).not.toBe(attempts[0]?.key);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('accepted-port-entry', { body: await page.screenshot(), contentType: 'image/png' });
  });
}
