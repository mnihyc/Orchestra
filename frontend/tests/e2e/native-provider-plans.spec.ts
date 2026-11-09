/** The existing provider card shows declared source plan names instead of native wire ids. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { scenario, signIn } from './support';

const PLANS = [
  ['pro', 'Pro (More)'], ['prolite', 'Pro'], ['promax', 'Pro (Max)'], ['ent26', 'Enterprise'],
  ['enterprise_cbp_automation', 'Enterprise (Automation)'],
  ['enterprise_cbp_usage_based', 'Enterprise CBP Usage Based'],
  ['sci', 'Academic Researchers'], ['sci_plus', 'Academic Researchers Plus'],
] as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a provider refresh retains its declared account plan ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const id = RUNS_ON.codexSubscription;
    const provider = planProvider({ id, name: 'Illustration account', harness_kind: 'codex' });
    const source = provider.sources.facts.descriptor;
    if (source?.facts === null || source?.facts === undefined) throw new Error('The native source fixture must declare its plan names');
    source.title = 'Codex subscription';
    source.facts.plans = Object.fromEntries(PLANS);
    provider.facts.plan = 'pro';
    const path = `/api/v1/providers/${id}`;
    await page.route((url) => url.pathname === path || url.pathname === `${path}/refresh`, async (route) => {
      await route.fulfill({ json: provider });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${id}?tab=overview`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const card = page.locator('[data-provider-usage="full"]');
    await expect(card.locator('[data-fact-member="plan"]')).toContainText('Pro (More)');
    for (const [plan, label] of PLANS) {
      provider.facts.plan = plan;
      provider.revision += 1;
      await card.getByRole('button', { name: words.providers.overview.refresh, exact: true }).click();
      const line = card.locator('[data-fact-member="plan"]');
      await expect(line.getByText(label, { exact: true })).toBeVisible();
      await expect(line.getByText(plan, { exact: true })).toHaveCount(0);
    }
    await page.reload();
    await expect(card.locator('[data-fact-member="plan"]')).toContainText('Academic Researchers Plus');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('native-plan-names', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
