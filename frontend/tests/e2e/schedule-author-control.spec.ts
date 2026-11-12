/** A schedule author can reduce future work while keeping only native thread Read. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { ThreadOpening, TopicSync } from '../../src/generated/api-types';
import { me, userRef } from '../doubles/fixtures';
import { inputSchedule } from '../doubles/input-schedules';
import { ACME, scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

// EN: Thread > Add a view > Schedules > Cancel; authorship survives lost interaction.
// 中文：线程 > 添加视图 > 计划 > 取消；失去交互权限后，作者仍可取消自己的计划。
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) test(`a read-only thread member cancels their authored schedule in ${locale}/${theme}/${width}`, async ({ page }, info) => {
  await scenario();
  await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
  // The shared sign-in starts in English; the ordinary interface control below chooses the case's locale afterward.
  await signIn(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  const words = locale === 'en' ? en : zh;
  let cancelled = false, writes = 0;
  const schedule = inputSchedule({ kind: undefined, agent: undefined, author: userRef(me(1, 'ada', 'Ada Lovelace')), name: 'My future review',
    destination: { new_thread: false, thread: { id: ACME.mainThread, kind: 'thread', name: 'Main', redacted: false } } });
  await page.route(`**/api/v1/threads/${ACME.mainThread}/opening**`, async (route) => {
    const response = await route.fetch(); const opening = await response.json() as ThreadOpening;
    opening.sync.thread.access.permissions = ['thread.view'];
    await route.fulfill({ response, json: opening });
  });
  await page.route('**/api/v1/topics/*/sync**', async (route) => {
    const response = await route.fetch(); const sync = await response.json() as TopicSync;
    sync.input_schedules = [{ ...schedule, state: cancelled ? 'deleted' : 'active', next_run_at: cancelled ? null : schedule.next_run_at }];
    await route.fulfill({ response, json: sync });
  });
  await page.route(`**/api/v1/input-schedules/${schedule.id}`, async (route) => {
    if (route.request().method() !== 'DELETE') { await route.continue(); return; }
    writes += 1; cancelled = true; await route.fulfill({ status: 204 });
  });
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Schedules');
  const table = page.getByRole('table', { name: words.agentSchedules.title, exact: true });
  const row = table.getByRole('row').filter({ hasText: schedule.name });
  const cancel = row.getByRole('button', { name: words.common.cancel, exact: true });
  await expect(cancel).toBeVisible(); await expect(cancel).not.toHaveAttribute('aria-disabled', 'true');
  await cancel.focus(); await page.keyboard.press('Enter');
  await expect(row).toContainText(words.agentSchedules.cancelled);
  expect(writes).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await info.attach('schedule-author-cancel', { body: await page.screenshot(), contentType: 'image/png' });
});
