/** Browser preview of the component contract; real resource/API/Ask integration has separate acceptance. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario } from './support';
import { language } from './ask-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`summary component refresh, failure, setup and Ask (${locale}/${theme}/${width})`, { tag: '@development' }, async ({ page, request }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    // EN: Save the actual profile language before mounting the preview; application bootstrap must agree.
    // 中文：先保存实际账户语言，再挂载预览，避免应用初始化把中文切回英文。
    await language(page, request, locale, theme);
    await page.reload();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await page.evaluate(async ({ locale, theme }) => {
      const path = '/tests/component/summary-box-harness.tsx';
      const { mountSummaryBox } = await import(/* @vite-ignore */ path) as { mountSummaryBox: (locale: 'en' | 'zh-CN', theme: 'light' | 'dark') => Promise<void> };
      await mountSummaryBox(locale, theme);
    }, { locale, theme });
    const words = (locale === 'en' ? english : chinese).summaryBox;
    const fixture = locale === 'en' ? {
      previous: 'The review is ready', updated: 'The review was accepted; publish next',
      complete: 'Complete refresh', fail: 'Fail refresh', noSummarizer: 'Show no summarizer', written: 'Written help',
      help: 'Set up your place, model and environment, then start shared work with an agent.',
    } : {
      previous: '审阅已就绪', updated: '审阅已通过，接下来发布',
      complete: '完成刷新', fail: '刷新失败', noSummarizer: '显示未设置摘要器', written: '人工帮助摘要',
      help: '准备所属位置、模型和环境，再与同事和代理开始协作。',
    };
    const frame = page.locator('[data-summary-preview]');
    const box = frame.locator('section');
    const refresh = frame.getByRole('button', { name: words.refresh, exact: true });
    await expect(box).toHaveAttribute('data-summary-state', 'fresh');
    await expect(box).toContainText(fixture.previous);
    await expect(frame.locator('output[aria-label="refresh requests"]')).toHaveText('0');
    await refresh.click();
    await expect(box).toHaveAttribute('data-summary-state', 'updating');
    await expect(box).toContainText(fixture.previous);
    await refresh.press('Enter');
    await expect(frame.locator('output[aria-label="refresh requests"]')).toHaveText('1');
    await frame.getByRole('button', { name: fixture.fail, exact: true }).click();
    await expect(box).toHaveAttribute('data-summary-state', 'failed');
    await expect(box.getByRole('alert')).toBeVisible();
    await expect(box).toContainText(fixture.previous);
    await refresh.click();
    await frame.getByRole('button', { name: fixture.complete, exact: true }).click();
    await expect(box).toHaveAttribute('data-summary-state', 'fresh');
    await expect(box).toContainText(fixture.updated);
    await box.getByRole('button', { name: words.ask, exact: true }).press('Enter');
    await expect(frame.locator('output[aria-label="Ask context"]')).toContainText(fixture.updated);
    await frame.getByRole('button', { name: fixture.noSummarizer, exact: true }).click();
    await expect(box).toHaveAttribute('data-summary-state', 'no_summarizer');
    await expect(refresh).toHaveCount(0);
    await box.getByRole('button', { name: words.choose, exact: true }).click();
    await expect(frame.locator('output[aria-label="configuration requests"]')).toHaveText('1');
    await box.getByRole('button', { name: words.ask, exact: true }).click();
    await expect(frame.locator('output[aria-label="Ask context"]')).toContainText('"summary":null');
    await frame.getByRole('button', { name: fixture.written, exact: true }).click();
    await expect(box).toContainText(words.written);
    await expect(box).toContainText(fixture.help);
    await expect(refresh).toHaveCount(0);
    await box.getByRole('button', { name: words.ask, exact: true }).click();
    await expect(frame.locator('output[aria-label="Ask context"]')).toContainText('/help/getting-started');
    const dimensions = await box.evaluate((element) => ({ left: element.getBoundingClientRect().left, right: element.getBoundingClientRect().right, width: element.clientWidth, scroll: element.scrollWidth }));
    expect(dimensions.left).toBeGreaterThanOrEqual(0);
    expect(dimensions.right).toBeLessThanOrEqual(width);
    expect(dimensions.scroll).toBeLessThanOrEqual(dimensions.width + 1);
    await info.attach('summary component', { body: await frame.screenshot(), contentType: 'image/png' });
  });
}
