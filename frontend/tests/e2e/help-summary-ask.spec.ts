/** Public guide/result actions enter the actual Ask composer; contract replies make no real model call. */
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { readArticle } from '../../src/help/guide';
import { scenario } from './support';
import { fixture, language } from './ask-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`written public summary and visible results seed Ask (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page);
    const t = await language(page, request, locale, theme);
    const file = locale === 'en' ? '../../../docs/user/getting-started.md' : '../../../docs/user/zh-CN/getting-started.md';
    const article = readArticle('getting-started', readFileSync(new URL(file, import.meta.url), 'utf8'));
    if (article.summary === undefined) throw new Error('A written guide summary is required');
    await page.goto('/help/getting-started');
    const region = page.getByRole('region', { name: article.title, exact: true });
    await expect(region.locator('[data-summary-text]')).toHaveText(article.summary);
    await info.attach('written help before Ask', { body: await page.screenshot(), contentType: 'image/png' });
    await region.getByRole('button', { name: t('summaryBox.ask'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    const input = panel.getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(input).toHaveValue(new RegExp('/help/getting-started'));
    expect(await input.inputValue()).toContain(`> ${article.summary}`);
    expect(server.sends).toEqual([]);
    await input.press('Enter');
    await expect.poll(() => server.sends.length).toBe(1);
    const message = server.sends[0]?.content.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n') ?? '';
    expect(message).toContain('/help/getting-started');
    expect(message).toContain(article.summary);
    await server.finish();
    await page.goto('/help?part=articles');
    await page.getByRole('searchbox', { name: t('help.search.label'), exact: true }).fill(`"${article.summary}"`);
    await expect(page.locator('[data-help-result="getting-started"]')).toHaveCount(1);
    await page.getByRole('button', { name: t('summaryBox.askResults'), exact: true }).click();
    await expect(input).toHaveValue(new RegExp(t('summaryBox.askResults')));
    expect(await input.inputValue()).toContain('/help/getting-started');
    expect(await input.inputValue()).toContain(article.summary);
    expect(server.sends).toHaveLength(1);
    await info.attach('visible public help results in Ask', { body: await panel.screenshot(), contentType: 'image/png' });
  });
}
