/** Local context refusals retain the original question and recovery choices in both editions. */
import { expect, test } from '@playwright/test';
import { fixture, language, openAsk } from './ask-support';
import { ACME, scenario } from './support';
import { ASK_ID } from '../unit/ask-fixtures';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  for (const code of ['ask_context_no_room', 'ask_context_too_long'] as const) test(`Ask context refusal: question → reason → new conversation (${code}, ${locale}, ${width}, ${theme})`, async ({ page, request }) => {
    // EN: topic > Ask > send > see the setting/model reason > New; the original
    // question stays in history and no second question is sent automatically.
    // 中文：话题 > 问一问 > 发送 > 阅读设置或模型容量原因 > 新建；原问题仍在历史中，
    // 不会自动重复发送。
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page), t = await language(page, request, locale, theme);
    await page.goto(`/topic/${ACME.fixLogin}`);
    await expect(page.getByRole('heading', { name: 'Fix login', exact: true, level: 1 })).toBeVisible();
    const panel = await openAsk(page, t);
    const question = locale === 'en' ? 'Continue from these earlier details' : '接着这些较早的细节说明';
    const box = panel.getByRole('textbox', { name: t('ask.composer') });
    await box.fill(question); await box.press('Enter');
    await expect.poll(() => server.sends.length).toBe(1);
    await server.contextRefusal(code, 'Chosen model');
    const reason = code === 'ask_context_no_room' ? t('ask.contextNoRoom') : t('ask.contextTooLong', { model: 'Chosen model' });
    await expect(panel.getByText(reason, { exact: true })).toBeVisible();
    await expect(panel.getByText(question, { exact: true })).toBeVisible();
    await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toHaveCount(0);
    await expect(panel.getByRole('button', { name: t('ask.keep'), exact: true })).toBeEnabled();
    expect(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await panel.getByRole('button', { name: t('ask.new'), exact: true }).click();
    await expect(box).toHaveValue('');
    expect(server.sends).toHaveLength(1);
    expect(server.record(ASK_ID).messages.some((message) => message.role === 'user' && message.content.some((part) => part.type === 'text' && part.text === question))).toBe(true);
  });
}
