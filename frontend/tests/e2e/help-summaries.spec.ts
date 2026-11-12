/** A person reads the same authored summary in the help index, search results and article. */
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { readArticle } from '../../src/help/guide';
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`written help summary from index through search and article (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const path = locale === 'en' ? '../../../docs/user/getting-started.md' : '../../../docs/user/zh-CN/getting-started.md';
    const article = readArticle('getting-started', readFileSync(new URL(path, import.meta.url), 'utf8'));
    const summary = article.summary;
    const section = article.headings.find((one) => one.id === 'choose-your-place');
    if (summary === undefined || section === undefined) throw new Error('The authored help summary and section are required');
    await page.goto('/help?part=articles');
    const entry = page.getByRole('button', { name: article.title, exact: true });
    await expect(entry).toHaveAccessibleDescription(summary);
    await entry.click();
    const body = page.getByRole('region', { name: article.title, exact: true });
    const title = body.getByRole('heading', { name: article.title, level: 1, exact: true });
    const paragraph = body.locator('[data-summary-text]');
    await expect(paragraph).toHaveText(summary);
    expect(await title.evaluate((element) => element.nextElementSibling?.textContent)).toBe(summary);
    await expect(body.getByRole('button', { name: words.summaryBox.refresh, exact: true })).toHaveCount(0);
    await page.goBack();
    const search = page.getByRole('searchbox', { name: words.help.search.label, exact: true });
    await search.fill(`"${summary}"`);
    const result = page.locator('[data-help-result="getting-started"]');
    await expect(result.getByRole('button', { name: article.title, exact: true })).toHaveAccessibleDescription(summary);
    await search.fill(section.text);
    const headingHit = result.getByRole('button', { name: section.text, exact: true });
    await expect(headingHit).toHaveAccessibleDescription(`${article.title} ${summary}`);
    await headingHit.click();
    await expect(body.getByRole('heading', { name: section.text, exact: true })).toBeInViewport();
    await expect(body.locator('[data-summary-text]')).toHaveText(summary);
    await page.goto('/help/getting-started');
    await expect(paragraph).toBeVisible();
    const dimensions = await paragraph.evaluate((element) => ({ left: element.getBoundingClientRect().left, right: element.getBoundingClientRect().right, width: element.clientWidth, scroll: element.scrollWidth }));
    expect(dimensions.left).toBeGreaterThanOrEqual(0);
    expect(dimensions.right).toBeLessThanOrEqual(width);
    expect(dimensions.scroll).toBeLessThanOrEqual(dimensions.width + 1);
    const image = info.outputPath(`written-help-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: image });
    await info.attach('written help summary', { path: image, contentType: 'image/png' });
  });
}
