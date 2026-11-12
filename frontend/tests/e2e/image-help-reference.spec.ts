/** Image choices have both authored reference editions and a real Help navigation path. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`image capability help click path ${locale}/${theme}/${width}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const optionTitle = locale === 'en' ? 'Option field reference' : '选项字段参考';
    const settingsTitle = locale === 'en' ? 'Settings reference' : '设置参考';
    const writes: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() !== 'GET' && /\/api\/v1\/(?:providers(?:\/|$)|nodes\/[^/]+\/settings(?:\/|$))/.test(path)) writes.push(path);
    });
    await page.goto('/help');
    await page.getByRole('tab', { name: new RegExp(`^${words.help.parts.articles}`) }).click();
    await page.getByRole('button', { name: new RegExp(`^${optionTitle}`) }).click();
    await expect(page.getByRole('heading', { name: optionTitle, exact: true, level: 1 })).toBeVisible();
    const prefix = 'option:protocols.openai-responses.native_tools.image_generation.request.';
    for (const field of ['action', 'background', 'model', 'output_compression', 'output_format', 'partial_images', 'quality', 'size']) {
      const cell = page.getByRole('cell', { name: `${prefix}${field}`, exact: true });
      await cell.scrollIntoViewIfNeeded();
      await expect(cell).toBeVisible();
    }
    await page.getByRole('link', { name: locale === 'en' ? 'settings reference' : '设置参考', exact: true }).click();
    await expect(page.getByRole('heading', { name: settingsTitle, exact: true, level: 1 })).toBeVisible();
    const backend = page.getByRole('cell', { name: 'setting:agents.image_generation_backend', exact: true }).locator('..');
    await backend.scrollIntoViewIfNeeded();
    await expect(backend).toContainText(locale === 'en' ? 'Unset selects no paid service' : '未设置时不选择付费服务');
    for (const field of ['config', 'offering', 'offering.offering', 'offering.provider', 'route']) {
      const cell = page.getByRole('cell', { name: `option:agents.image_generation_backend.${field}`, exact: true });
      await cell.scrollIntoViewIfNeeded();
      await expect(cell).toBeVisible();
    }
    expect(writes).toEqual([]);
    // EN: Help → Articles → Option field reference → image tool fields → Settings reference → image backend and its five fields.
    // 中文：“帮助” → “文章” → “选项字段参考” → 图像工具字段 → “设置参考” → 图像后端及其五个字段。
  });
}
