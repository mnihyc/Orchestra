/** An unresolved answer retry remains one native input intent after a lost acknowledgement. */
import { expect, test } from '@playwright/test';
import type { AskMessageBody } from '../../src/generated/api-types';
import { ASK_ID, ASK_TURN, askSnapshot } from '../unit/ask-fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario } from './support';

// EN: topic > Ask > answer Retry > lost acknowledgement > same answer Retry.
// 中文：话题 > 问一问 > 重试回答 > 确认响应丢失 > 再次重试同一回答。
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) test(`Ask row retry retains an unresolved client identity (${locale}/${theme}/${width})`, async ({ page, request }) => {
  await scenario(); await page.setViewportSize({ width, height: 1000 });
  await fixture(page, [askSnapshot()]);
  const t = await language(page, request, locale, theme);
  const attempts: { client_id: string; content: unknown }[] = [], accepted = new Set<string>();
  await page.route(`**/api/v1/ask/conversations/${ASK_ID}/messages`, async (route) => {
    const body: unknown = route.request().postDataJSON();
    if (typeof body !== 'object' || body === null || !('client_id' in body)
      || typeof body.client_id !== 'string' || !('content' in body)) {
      throw new Error('The retry must send its actual input identity and content');
    }
    const content: unknown = body.content;
    if (!Array.isArray(content)) throw new Error('The retry content must be a part list');
    attempts.push({ client_id: body.client_id, content: body.content }); accepted.add(body.client_id);
    if (attempts.length === 1) { await route.abort('connectionfailed'); return; }
    await route.fulfill({ status: 202, json: { conversation_id: ASK_ID, turn_id: ASK_TURN, seq: 5 } });
  });
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  const panel = page.locator('[data-view-window="ask"]');
  await expect(panel.getByText('Read the release notes.', { exact: true })).toBeVisible();
  const retry = panel.getByRole('button', { name: t('ask.retry'), exact: true });
  await retry.focus(); await page.keyboard.press('Enter');
  await expect(panel.getByRole('alert')).toBeVisible();
  await expect(retry).toBeEnabled();
  await retry.focus(); await page.keyboard.press('Enter');
  await expect.poll(() => attempts.length).toBe(2);
  expect(attempts[1]?.client_id).toBe(attempts[0]?.client_id);
  expect(attempts[1]?.content).toEqual(attempts[0]?.content);
  expect(accepted.size).toBe(1);
  // Once acknowledged, a later explicit retry remains a new generation act.
  await expect(retry).toBeEnabled();
  await retry.focus(); await page.keyboard.press('Enter');
  await expect.poll(() => attempts.length).toBe(3);
  expect(attempts[2]?.client_id).not.toBe(attempts[0]?.client_id);
  expect(accepted.size).toBe(2);
});
