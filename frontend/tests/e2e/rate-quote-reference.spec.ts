/** Rate-card fields remain discoverable in the authored reference in both languages. */
import {expect, test} from '@playwright/test';
import english from '../../src/i18n/en.json' with {type: 'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type: 'json'};
import {scenario, signIn} from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`${locale}/${theme}: rate-card help explains hourly cache and actual capacity quotes`, async ({page}, info) => {
    await scenario();
    await page.emulateMedia({colorScheme: theme});
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', {name: /^(Language|语言)$/}).click();
      await page.getByRole('option', {name: '简体中文', exact: true}).click();
    }
    const words = locale === 'en' ? english : chinese;
    const title = locale === 'en' ? 'Settings reference' : '设置参考';
    await page.goto('/help');
    await page.getByRole('searchbox', {name: words.help.search.label}).fill(title);
    await page.getByRole('button', {name: new RegExp(`^${title}`)}).click();
    await expect(page).toHaveURL(/\/help\/settings-reference$/);
    await expect(page.getByRole('heading', {name: title, level: 1})).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    for (const width of [1280, 400]) {
      await page.setViewportSize({width, height: 900});
      for (const field of ['cache_write_1h_per_mtok', 'above.*.cache_write_1h_per_mtok', 'cache_write_12h_per_mtok', 'above.*.cache_write_12h_per_mtok', 'capacity_tiers']) {
        const row = page.getByRole('row').filter({has: page.getByText(`option:usage.rates.*.${field}`, {exact: true})});
        await row.scrollIntoViewIfNeeded();
        await expect(row).toBeVisible();
        await expect(row).toContainText(field === 'above.*.cache_write_12h_per_mtok'
          ? (locale === 'en' ? 'root quote applies' : '继承根报价') : field === 'capacity_tiers'
          ? (locale === 'en' ? 'actually reported' : '实际报告')
          : (locale === 'en' ? 'absent quote stays unknown' : '缺少报价时仍为未知'));
      }
      await info.attach(`rate-quotes-${width}`, {body: await page.screenshot(), contentType: 'image/png'});
    }
  });
}
