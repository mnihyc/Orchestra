/** Row Retry shares the composer's retained input intent through window changes and acknowledgement loss. */
import { expect, test } from '@playwright/test';
import type { AskMessageBody } from '../../src/generated/api-types';
import { ASK_ID, ASK_TURN, askSnapshot } from '../unit/ask-fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`Ask row Retry keeps one uncertain turn and allows a new acknowledged Retry (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > keyboard Retry answer > lose accepted acknowledgement > Pop out > Retry > acknowledge > Dock > Retry as a new intent.
    // 中文：话题 > 问一问 > 用键盘重新回答 > 已受理的确认丢失 > 弹出窗口 > 再次重新回答 > 确认成功 > 停靠 > 主动生成新回答。
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page, [askSnapshot()]); const t = await language(page, request, locale, theme);
    const attempts: AskMessageBody[] = [], admitted = new Set<string>();
    await page.route(`**/api/v1/ask/conversations/${ASK_ID}/messages`, async (route) => {
      const body = route.request().postDataJSON() as AskMessageBody; attempts.push(body); admitted.add(body.client_id);
      if (attempts.length === 1) { await route.abort('connectionfailed'); return; }
      await route.fulfill({ status: 202, json: { conversation_id: ASK_ID, turn_id: ASK_TURN, seq: 5 } });
    });
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]'), retry = panel.getByRole('button', { name: t('ask.retry'), exact: true });
    await expect(retry).toBeEnabled(); await retry.focus();
    await page.screenshot({ path: info.outputPath('ask-row-before-retry.png'), animations: 'disabled' });
    await page.keyboard.press('Enter');
    await expect(panel.getByRole('alert')).toBeVisible(); await expect(retry).toBeEnabled();
    await panel.getByRole('button', { name: t('viewWindow.popOut'), exact: true }).click();
    await expect(panel).toHaveAttribute('data-floating', 'true');
    await retry.focus(); await page.keyboard.press('Enter'); await expect.poll(() => attempts.length).toBe(2);
    expect(attempts[1]).toEqual(attempts[0]); expect(admitted.size).toBe(1);
    await expect(retry).toBeEnabled();
    await panel.getByRole('button', { name: t('viewWindow.dock'), exact: true }).click();
    await retry.focus(); await page.keyboard.press('Enter'); await expect.poll(() => attempts.length).toBe(3);
    expect(attempts[2]?.client_id).not.toBe(attempts[0]?.client_id); expect(admitted.size).toBe(2);
    expect(attempts[2]?.content).toEqual(attempts[0]?.content);
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('ask-row-after-confirmed-retry.png'), animations: 'disabled' });
  });
}
