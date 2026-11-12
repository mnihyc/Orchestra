/** A recorded task remains visible when this exact native build declares no End control. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { backgroundRoster, backgroundWake, backgroundWork } from '../doubles/background-wake.ts';
import { ACME, DOUBLE, emit, scenario, signIn } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`unoffered Codex terminal End has its cause ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }) });
    expect(appearance.ok).toBe(true);
    const words = locale === 'en' ? english : chinese;
    const reason = 'This Codex version cannot end background terminals.';
    const label = 'Fixture background command';
    const value = backgroundRoster({ supported: false, state: 'unavailable', reason, total: null, items: [], can_end: false,
      wake: backgroundWake({ tasks: [backgroundWork({ label, can_end: false, end_id: null })] }) });
    await emit(ACME.mainThread, { background: value });
    await page.route(`**/api/v1/threads/${ACME.mainThread}/background*`, route => route.fulfill({ json: value }));
    await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]');
    await expect(header.getByText(label, { exact: true })).toBeVisible();
    const cause = words.running.unavailableWithReason.replace('{{reason}}', reason);
    await expect(header.getByText(cause, { exact: true })).toBeVisible();
    await expect(header.getByRole('button', { name: words.backgroundWake.endNamed.replace('{{name}}', label), exact: true })).toHaveCount(0);
    await header.getByRole('button', { name: words.running.title, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.running.title, exact: true });
    await expect(dialog.getByText(cause, { exact: true })).toBeVisible();
    await expect(dialog.getByRole('checkbox')).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: words.running.end, exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('codex-terminal-end-unavailable.png') });
  });
}
