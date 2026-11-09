/** A reader opens the Ask guide from its panel, follows its links and keeps an unsent question. */
import { expect, test } from '@playwright/test';
import { ACME, scenario } from './support';
import { fixture, language } from './ask-support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`Ask help is reachable and free of model calls (${locale}, ${width}, ${theme})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page); const t = await language(page, request, locale, theme), writes: string[] = [];
    page.on('request', (request) => { if (request.method() === 'POST' && /\/ask\/conversations\/[^/]+\/messages$/.test(new URL(request.url()).pathname)) writes.push(request.url()); });
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    await panel.getByRole('textbox', { name: t('ask.composer') }).fill('Keep my unsent question');
    if (theme === 'dark') await panel.getByRole('button', { name: t('viewWindow.popOut'), exact: true }).click();
    await panel.getByRole('button', { name: t('help.hint.label', { term: t('ask.title') }), exact: true }).click();
    const hint = page.getByRole('dialog', { name: t('ask.title'), exact: true });
    await expect(hint).toContainText(t('ask.helpWhat'));
    await hint.getByRole('button', { name: t('ask.guide'), exact: true }).click();
    await expect(page.getByRole('heading', { name: t('ask.guide'), level: 1, exact: true })).toBeVisible();
    const article = page.getByRole('region', { name: t('ask.guide'), exact: true });
    await expect(article.locator('[id="open-ask"]')).toBeInViewport();
    await article.locator('[id="privacy-cost-and-retention"]').scrollIntoViewIfNeeded();
    await expect(article.locator('[id="privacy-cost-and-retention"]')).toBeInViewport();
    await expect(article).toContainText(locale === 'en' ? '30 days after its last message' : '最后一条消息发出 30 天后永久删除');
    await article.locator('[id="installation-addresses"]').scrollIntoViewIfNeeded(); await expect(article).toContainText('alternate_urls');
    await page.screenshot({ path: info.outputPath(`ask-guide-${locale}-${width}-${theme}.png`), animations: 'disabled' });
    await article.getByRole('link', { name: locale === 'en' ? 'following a running thread' : '关注正在运行的线程', exact: true }).click();
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Follow a running thread' : '关注正在运行的线程', level: 1, exact: true })).toBeVisible();
    if (await page.locator('[data-help-reading]').isVisible()) await page.getByRole('button', { name: t('helpFlow.back'), exact: true }).click();
    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    await expect(panel.getByRole('textbox', { name: t('ask.composer') })).toHaveValue('Keep my unsent question');
    expect(writes).toEqual([]);
  });
}
