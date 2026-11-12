/** A joined thread keeps its shared-session billing explanation beside the actual wake setting. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { backgroundRoster, backgroundWake, backgroundWork } from '../doubles/background-wake';
import { ACME, DOUBLE, emit, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`background sharing remains visible on the thread click path (${locale}/${theme})`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }),
      signal: AbortSignal.timeout(10_000),
    });
    expect(appearance.ok).toBe(true);
    const words = (locale === 'en' ? english : chinese).backgroundWake;
    await emit(ACME.mainThread, { background: backgroundRoster({ wake: backgroundWake({ tasks: [backgroundWork({ label: 'Build web' })] }) }) });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
    const header = page.locator('[data-thread-header]');
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(header.getByText('Build web', { exact: true })).toBeVisible();
      await expect(header.getByText(words.willWake, { exact: true })).toBeVisible();
      await expect(header.getByText(words.overlapBilling, { exact: true })).toBeVisible();
      await header.getByRole('button', { name: words.settingOn, exact: true }).click();
      await expect(page.getByRole('dialog', { name: words.settingTitle, exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog', { name: words.settingTitle, exact: true })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`background-sharing-${width}.png`) });
    }
  });
}
