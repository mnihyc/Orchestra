/** The preset's public credential hints have real bilingual field references in Help. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

const references = ['providers.presets.*.credential_hints', ...['field', 'prefix', 'role'].map((field) => `providers.presets.*.credential_hints.*.${field}`)];
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`credential hint references ${language}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    const title = language === 'en' ? 'Option field reference' : '选项字段参考';
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 }); await page.goto('/help?part=articles');
      await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill(title);
      await page.locator('[data-help-result="option-reference"]').getByRole('button', { name: title, exact: true }).click();
      const article = page.getByRole('region', { name: title, exact: true });
      for (const reference of references) {
        const entry = article.getByText(`option:${reference}`, { exact: true });
        await entry.scrollIntoViewIfNeeded(); await expect(entry).toBeVisible();
        await expect(entry.locator('xpath=ancestor::tr')).toContainText(language === 'en' ? /propose|role|source|key/ : /建议|用途|数据源|前缀/);
      }
      await info.attach(`credential-references-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
