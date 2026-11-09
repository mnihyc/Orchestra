/** The actual open-side menu keeps Move visible, inert and explained without a help block. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { screenReady } from '../../e2e/layout-rules-helpers';
import type { ThreadOpening, ThreadSync } from '../../src/generated/api-types';
import { MAIN } from './change-support';
import { emit, playScene, scenario } from './support';

for (const [locale, label, reason] of [
  ['en', 'Move to another org', 'Moves with its main thread · Keep it to move it alone'],
  ['zh-CN', '移到其他组织', '随主会话一起移动 · 保留后可单独移动'],
] as const) test(`an open side exposes only the disabled Move tooltip (${locale})`, async ({ page }, info) => {
  // EN: Main > Side thread > start > open the actual side page > … > disabled Move > hover > Enter/click do nothing.
  // 中文：主会话 > 旁路会话 > 开始 > 打开真实旁路页面 > … > 已禁用的移动 > 悬停 > 回车和点击均不操作。
  await scenario();
  await emit(MAIN, { thread: { state: 'idle' } });
  await playScene(MAIN, 'history');
  await signInScreenRoute(page, { locale, theme: 'light', width: 1440 });
  const words = (key: string) => caption(`@${key}`, locale);
  let side: string | null = null;
  // The side double supplies its own authorized sync. Only the aggregate supplements are fixture-empty.
  await page.route('**/api/v1/threads/*/opening?*', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.split('/')[4] !== side) { await route.continue(); return; }
    url.pathname = url.pathname.replace(/\/opening$/u, '/sync');
    const response = await route.fetch({ url: url.toString() });
    expect(response.ok()).toBe(true);
    const sync = await response.json() as ThreadSync;
    const opening: ThreadOpening = { sync, draft: null, parameters: null, limits: null, settings: [],
      outline: { entries: [], next_before: null, seq: sync.seq } };
    await route.fulfill({ json: opening });
  });
  await page.goto(`/thread/${MAIN}`);
  await screenReady(page, locale, 'light');
  await page.locator('[data-thread-header]').getByRole('button', { name: words('side.menu.ask'), exact: true }).click();
  const ask = page.getByRole('dialog', { name: words('side.ask.title'), exact: true });
  await ask.getByRole('textbox', { name: words('side.ask.label'), exact: true }).fill('A question to keep on the side');
  const made = page.waitForResponse((response) => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `/api/v1/threads/${MAIN}/side`);
  await ask.getByRole('button', { name: words('side.ask.send'), exact: true }).click();
  const accepted = await made;
  expect(accepted.status()).toBe(201);
  const body = await accepted.json() as { thread?: { id?: unknown } };
  if (typeof body.thread?.id !== 'string') throw new Error('The native-shaped side receipt omitted its actual thread');
  side = body.thread.id;
  const writes: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== 'GET' && /\/(?:move-out|forks|binding|home)(?:\/|$)/u.test(path)) writes.push(path);
  });
  await page.goto(`/thread/${side}`);
  await screenReady(page, locale, 'light');
  await page.locator('[data-thread-header]:visible [data-thread-more]').click();
  const move = page.getByRole('menuitem', { name: label, exact: true });
  await expect(move).toHaveAttribute('aria-disabled', 'true');
  await expect(move).toHaveText(label);
  await expect(move.getByText(reason, { exact: true })).toHaveCount(0);
  await move.hover();
  await expect(page.locator('[data-tooltip-content]').filter({ hasText: reason })).toBeVisible();
  await page.screenshot({ path: info.outputPath('open-side-move-tooltip.png'), animations: 'disabled' });
  await move.click({ force: true });
  await move.focus();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/thread/${side}(?:[?#]|$)`, 'u'));
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(writes).toEqual([]);
});
