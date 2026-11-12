/** A shared cap interruption refreshes current controls without losing the partial answer. */
import { expect, test } from '@playwright/test';
import { fixture, language, openAsk } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`Ask refreshes after its active turn is stopped by a cap (${locale})`, async ({ page, request }) => {
  // EN: topic > Ask > send a question > while the partial answer streams, an
  // admin lowers its person's Ask cap with Stop > panel refreshes > Stop goes
  // away and the partial answer remains. Native API case checks the actual cap.
  // 中文：话题 > 问一问 > 提问 > 部分回答流式显示时，管理员下调提问者额度并选择停止 >
  // 面板刷新，停止按钮消失，已写的回答仍保留。原生 API 用例验证实际额度与中断。
  await scenario();
  const server = await fixture(page), t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login', exact: true, level: 1 })).toBeVisible();
  const panel = await openAsk(page, t);
  const box = panel.getByRole('textbox', { name: t('ask.composer') });
  await box.fill('Explain this topic within my allowance'); await box.press('Enter');
  await expect.poll(() => server.sends.length).toBe(1);
  await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toBeVisible();
  const partial = locale === 'en' ? 'The first finding is retained.' : '保留已得到的首项结论。';
  await server.delta(partial); await expect(panel.getByText(partial, { exact: true })).toBeVisible();
  const reason = locale === 'en' ? 'The Ask allowance was reached.' : '提问额度已用尽。';
  await server.capStop(reason);
  await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toHaveCount(0);
  await expect(panel.getByText(partial, { exact: true })).toBeVisible();
  await expect(panel.getByText(reason, { exact: true })).toBeVisible();
  expect(server.stops).toBe(0);
});
