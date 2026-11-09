/** Catalogue key checking changes compilation; real interpolation and scoped labels remain bilingual. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`profile home link and search interpolation after the typing change in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto('/u/ada');
    const home = page.getByRole('link', { name: words.profile.page.personalOrganization.replace('{{name}}', 'Ada Lovelace'), exact: true });
    await expect(home).toBeVisible(); await home.click(); await expect(page).toHaveURL(/\/org\/ada/);
    await page.goto('/search?q=NoSuchCatalogueBoundaryResult');
    await expect(page.getByRole('button', { name: `${words.search.org.label}: ${words.search.org.in.replace('{{org}}', 'Ada Lovelace')}` })).toBeVisible();
    const total = locale === 'en' ? english.search.total_other : chinese.search.total_other;
    await expect(page.getByRole('status').filter({ hasText: total.replace('{{number}}', '0') })).toBeVisible();
    await expect(page.getByRole('region', { name: words.search.results })).toHaveAttribute('aria-busy', 'false');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
