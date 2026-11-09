/** The error-row button repeats exactly Send, Stop or draft preparation through the real Ask composer. */
import { expect, test } from '@playwright/test';
import type { AskCreateBody, AskMessageBody } from '../../src/generated/api-types';
import { ASK_ID, ASK_TURN, askConversation, askSnapshot } from '../unit/ask-fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`Ask Try again repeats the failed Send (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > type > Send loses its acknowledgement > edit the draft > keyboard Try again > original request succeeds, later edit stays unsent.
    // 中文：话题 > 问一问 > 输入 > 发送确认丢失 > 修改草稿 > 用键盘重试 > 原请求成功，后续修改仍未发送。
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page, [askSnapshot()]), t = await language(page, request, locale, theme);
    const sends: AskMessageBody[] = [], accepted = new Set<string>();
    await page.route(`**/api/v1/ask/conversations/${ASK_ID}/messages`, async (route) => {
      const body = route.request().postDataJSON() as AskMessageBody; sends.push(body); accepted.add(body.client_id);
      if (sends.length === 1) { await route.abort('connectionfailed'); return; }
      await route.fulfill({ status: 202, json: { conversation_id: ASK_ID, turn_id: ASK_TURN, seq: 5 } });
    });
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]'), box = panel.getByRole('textbox', { name: t('ask.composer') });
    await box.fill('Send the original question'); await box.press('Enter');
    const retry = panel.getByRole('button', { name: t('common.tryAgain'), exact: true }); await expect(retry).toBeVisible();
    await box.fill('Keep this later edit unsent');
    await page.screenshot({ path: info.outputPath('ask-send-before-recovery.png'), animations: 'disabled' });
    await retry.focus(); await page.keyboard.press('Enter');
    await expect.poll(() => sends.length).toBe(2); expect(sends[1]).toEqual(sends[0]); expect(accepted.size).toBe(1);
    await expect(retry).toHaveCount(0); await expect(box).toHaveValue('Keep this later edit unsent');
    expect(server.creates).toEqual([]); expect(server.stops).toBe(0);
    await page.screenshot({ path: info.outputPath('ask-send-after-recovery.png'), animations: 'disabled' });
  });

  test(`Ask Try again repeats the failed Stop (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    // EN: Topic > Ask running answer > Stop fails > keyboard Try again > Stop is reissued; no message or allocation is submitted.
    // 中文：话题 > 问一问正在回答 > 停止失败 > 用键盘重试 > 再次停止；不会发送消息或新建对话。
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page, [askSnapshot({ conversation: askConversation({ state: 'running' }) })]);
    const t = await language(page, request, locale, theme); let stops = 0;
    await page.route(`**/api/v1/ask/conversations/${ASK_ID}/stop`, async (route) => {
      stops += 1; if (stops === 1) { await route.abort('connectionfailed'); return; }
      await route.fallback();
    });
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    await panel.getByRole('button', { name: t('composer.stop'), exact: true }).click();
    const retry = panel.getByRole('button', { name: t('common.tryAgain'), exact: true }); await expect(retry).toBeVisible();
    await page.screenshot({ path: info.outputPath('ask-stop-before-recovery.png'), animations: 'disabled' });
    await retry.focus(); await page.keyboard.press('Enter');
    await expect.poll(() => stops).toBe(2); await expect(retry).toHaveCount(0);
    expect(server.sends).toEqual([]); expect(server.creates).toEqual([]); expect(server.stops).toBe(1);
    await page.screenshot({ path: info.outputPath('ask-stop-after-recovery.png'), animations: 'disabled' });
  });

  test(`Ask Try again prepares a failed draft without sending it (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > type without sending > draft preparation fails > keyboard Try again > one prepared conversation, same unsent draft and no Send.
    // 中文：话题 > 问一问 > 只输入不发送 > 草稿准备失败 > 用键盘重试 > 对话准备完成，草稿仍未发送。
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page), t = await language(page, request, locale, theme);
    const allocations: { key: string | undefined; body: AskCreateBody }[] = [];
    await page.route((url) => url.pathname === '/api/v1/ask/conversations', async (route) => {
      const request = route.request(); if (request.method() !== 'POST') { await route.fallback(); return; }
      allocations.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() as AskCreateBody });
      if (allocations.length === 1) { await route.abort('connectionfailed'); return; }
      await route.fallback();
    });
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]'), box = panel.getByRole('textbox', { name: t('ask.composer') });
    await box.fill('Keep this question unsent');
    const retry = panel.getByRole('button', { name: t('common.tryAgain'), exact: true }); await expect(retry).toBeVisible();
    await page.screenshot({ path: info.outputPath('ask-allocation-before-recovery.png'), animations: 'disabled' });
    await retry.focus(); await page.keyboard.press('Enter');
    await expect.poll(() => allocations.length).toBe(2); expect(allocations[0]?.key).toBeTruthy(); expect(allocations[1]).toEqual(allocations[0]);
    await expect(retry).toHaveCount(0); await expect(box).toHaveValue('Keep this question unsent');
    expect(server.creates).toHaveLength(1); expect(server.sends).toEqual([]); expect(server.stops).toBe(0);
    await page.screenshot({ path: info.outputPath('ask-allocation-after-recovery.png'), animations: 'disabled' });
  });
}
