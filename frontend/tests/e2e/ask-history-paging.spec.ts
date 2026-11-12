/** A reader reaches every older message through actual paging controls in either language. */
import { expect, test } from '@playwright/test';
import { ASK_ID, askConversation, askMessage, askSnapshot } from '../unit/ask-fixtures';
import { id } from '../doubles/fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`Ask reaches the beginning without skipping or repeating a page (${locale})`, async ({ page, request }) => {
  // EN: topic > Ask > Load earlier messages > Load earlier messages > first
  // question. Each earlier answer appears once and the paging control ends.
  // 中文：话题 > 问一问 > 加载更早消息 > 再次加载 > 首个问题；每条早先回答只出现一次，
  // 到开头后不再显示加载控件。
  await scenario();
  const conversation = askConversation({ seq: 50 });
  const latest = askSnapshot({ conversation, messages: [
    askMessage({ id: id(0x74, 20), role: 'user', content: [{ type: 'text', text: 'Newest question' }] }),
    askMessage({ id: id(0x74, 21), content: [{ type: 'text', text: 'Newest answer' }] }),
  ], previous_cursor: '200' });
  const server = await fixture(page, [latest]);
  server.olderPage('200', askSnapshot({ conversation, messages: [
    askMessage({ id: id(0x74, 10), role: 'user', content: [{ type: 'text', text: 'Middle question' }] }),
    askMessage({ id: id(0x74, 11), content: [{ type: 'text', text: 'Middle answer' }] }),
  ], previous_cursor: '100' }));
  server.olderPage('100', askSnapshot({ conversation, messages: [
    askMessage({ id: id(0x74, 1), role: 'user', content: [{ type: 'text', text: 'First question' }] }),
    askMessage({ id: id(0x74, 2), content: [{ type: 'text', text: 'First answer' }] }),
  ], previous_cursor: null }));
  const t = await language(page, request, locale, 'light'), cursors: string[] = [];
  page.on('request', (outgoing) => {
    const url = new URL(outgoing.url()), before = url.searchParams.get('before');
    if (outgoing.method() === 'GET' && url.pathname.endsWith(`/ask/conversations/${ASK_ID}`) && before !== null) cursors.push(before);
  });
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  const panel = page.locator('[data-view-window="ask"]');
  await expect(panel.getByText('Newest answer', { exact: true })).toBeVisible();
  await panel.getByRole('button', { name: t('ask.older'), exact: true }).click();
  await expect(panel.getByText('Middle answer', { exact: true })).toBeVisible();
  await panel.getByRole('button', { name: t('ask.older'), exact: true }).click();
  await expect(panel.getByText('First question', { exact: true })).toBeVisible();
  for (const answer of ['First answer', 'Middle answer', 'Newest answer']) await expect(panel.getByText(answer, { exact: true })).toHaveCount(1);
  await expect(panel.getByRole('button', { name: t('ask.older'), exact: true })).toHaveCount(0);
  expect(cursors).toEqual(['200', '100']);
});
