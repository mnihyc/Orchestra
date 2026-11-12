/** The shipped guide explains partial import retries without starting another import. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`text import retry guide ${language}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    const title = language === 'en' ? 'Import and export' : '导入和导出';
    const imports: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/documents/import')) imports.push(request.url());
    });
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 }); await page.goto('/help?part=articles');
      await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill(title);
      await page.locator('[data-help-result="import-and-export"]').getByRole('button', { name: title, exact: true }).click();
      const article = page.getByRole('region', { name: title, exact: true });
      await expect(article.getByRole('heading', { name: title, level: 1, exact: true })).toBeVisible();
      const paragraph = article.locator('p').filter({ hasText: language === 'en' ? 'For text-file API imports' : '通过文本文件 API 导入时' });
      await paragraph.scrollIntoViewIfNeeded();
      await expect(paragraph).toContainText('client_id'); await expect(paragraph).toContainText('Idempotency-Key');
      await expect(paragraph).toContainText('complete: false');
      await expect(paragraph).toContainText(language === 'en' ? 'never recreated' : '不会重新创建');
      const bounds = await paragraph.boundingBox(); if (bounds === null) throw new Error('the actual import retry paragraph is not visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`import-retry-guide-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
    expect(imports).toEqual([]);
  });
}
