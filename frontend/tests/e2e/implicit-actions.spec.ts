/** Known actions keep their actual reason across the composer, visible rows and Ask header. */
import { expect, type Locator, type Page, test, type TestInfo } from '@playwright/test';
import { askConversation, ASK_ID } from '../unit/ask-fixtures';
import { fixture, language } from './ask-support';
import { begin, MAIN } from './inspector-support';
import { ACME, DOUBLE, emit, PEOPLE, scenario } from './support';

async function photograph(page: Page, info: TestInfo, name: string) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath(`${name}.png`), animations: 'disabled' });
}

async function reason(page: Page, button: Locator, words: string) {
  await expect(button).toHaveAttribute('aria-disabled', 'true');
  await button.focus(); await expect(page.getByRole('tooltip')).toContainText(words);
  await page.keyboard.press('Escape'); await button.blur();
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Thread → /goal → archived/side reason → click or Enter creates nothing; the same thread-menu entry explains why.
  // 线程 → /goal → 查看归档或旁路原因 → 点击或回车均不创建目标；线程菜单显示相同原因。
  test(`goal availability stays visible (${locale}, ${width}, ${theme})`, async ({ page, request }, info) => {
    await begin(''); await emit(MAIN, { thread: { goal: null, archived: false, side_of: null } });
    const t = await language(page, request, locale, theme);
    await page.setViewportSize({ width, height: 900 }); await page.goto(`/thread/${MAIN}`);
    const writes: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith(`/threads/${MAIN}/goal`)) writes.push(request.url());
    });
    const box = page.getByRole('textbox', { name: t('composer.label'), exact: true });
    await box.fill('/goal'); const command = page.getByRole('option').filter({ hasText: '/goal' });
    await expect(command).not.toHaveAttribute('aria-disabled', 'true'); await photograph(page, info, 'goal-before');
    for (const state of ['archived', 'side'] as const) {
      const off = t(state === 'archived' ? 'goals.act.archived' : 'goals.act.sideThread');
      await emit(MAIN, { thread: { archived: state === 'archived', side_of: state === 'side' ? { id: ACME.repro, kind: 'thread', name: 'Repro', redacted: false } : null } });
      await expect(command).toHaveAttribute('aria-disabled', 'true'); await expect(command).toContainText(off);
      await command.click({ force: true }); await box.press('Enter');
      await expect(box).toHaveValue('/goal'); await expect(page.getByRole('dialog', { name: new RegExp(t('goals.editor.titleSet')) })).toHaveCount(0);
      await photograph(page, info, `goal-${state}`);
      await page.locator('[data-thread-header]').getByRole('button', { name: t('places.menu.more', { name: 'Main thread' }), exact: true }).click();
      const entry = page.getByRole('menuitem', { name: t('goals.menu.set'), exact: true });
      await expect(entry).toHaveAttribute('aria-disabled', 'true'); await expect(entry).toContainText(off);
      await page.keyboard.press('Escape');
    }
    expect(writes).toEqual([]);
  });

  // Visible thread row → Actions → checking → current refusal → Request access; no Export request is sent.
  // 可见线程行 → 操作菜单 → 正在检查 → 当前拒绝原因 → 申请权限；不发起导出请求。
  test(`row Export explains checking and refusal (${locale}, ${width}, ${theme})`, async ({ page, request }, info) => {
    await scenario();
    expect((await request.post(`${DOUBLE}/__double/export/bar`, { data: { username: 'dora', permission: 'thread.export', object: ACME.repro } })).ok()).toBe(true);
    const t = await language(page, request, locale, theme, PEOPLE.dora);
    await page.setViewportSize({ width, height: 900 }); await page.goto(`/thread/${ACME.mainThread}`);
    let release: () => void = () => undefined;
    const ready = new Promise<void>((resolve) => { release = resolve; });
    await page.route(`**/api/v1/nodes/${ACME.repro}`, async (route) => { await ready; await route.fallback(); });
    const exports: string[] = [];
    page.on('request', (request) => {
      if (new URL(request.url()).pathname.endsWith(`/threads/${ACME.repro}/export`)) exports.push(request.url());
    });
    await photograph(page, info, 'row-before');
    try {
      await page.getByRole('button', { name: /Repro/ }).first().click();
      const entry = page.getByRole('menuitem', { name: t('export.menu.export'), exact: true });
      await expect(entry).toHaveAttribute('aria-disabled', 'true'); await expect(entry).toContainText(t('export.menu.checking'));
      await expect(page.getByRole('menuitem', { name: t('export.menu.request'), exact: true })).toHaveCount(0);
      await photograph(page, info, 'row-checking');
    } finally { release(); }
    const entry = page.getByRole('menuitem', { name: t('export.menu.export'), exact: true });
    await expect(entry).toContainText(t('export.menu.notAllowed', { name: 'Ada Lovelace' }));
    await entry.click({ force: true }); expect(exports).toEqual([]);
    await photograph(page, info, 'row-refused');
    await page.getByRole('menuitem', { name: t('export.menu.request'), exact: true }).click();
    await expect(page.getByRole('dialog').filter({ hasText: 'Repro' })).toBeVisible();
    expect(exports).toEqual([]);
  });

  // Topic → Ask → focus empty header acts → ask a question → Keep explains running while Export still works → native refusal.
  // 话题 → 问一问 → 聚焦尚无对话的操作 → 提问 → 保留说明回答进行中，导出仍可用 → 显示服务端拒绝原因。
  test(`Ask header reasons keep Export and Keep independent (${locale}, ${width}, ${theme})`, async ({ page, request }, info) => {
    await scenario(); const server = await fixture(page), t = await language(page, request, locale, theme);
    await page.setViewportSize({ width, height: 900 }); await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    const keeping = panel.getByRole('button', { name: t('ask.keep'), exact: true });
    const exporting = panel.getByRole('button', { name: t('ask.export'), exact: true });
    await reason(page, keeping, t('ask.actions.noConversation')); await reason(page, exporting, t('ask.actions.noConversation'));
    await photograph(page, info, 'ask-before');
    const box = panel.getByRole('textbox', { name: t('ask.composer'), exact: true });
    await box.fill('Explain the release changes'); await box.press('Enter');
    await expect(panel.getByText(t('ask.running'), { exact: true })).toBeVisible();
    await reason(page, keeping, t('ask.actions.answerRunning'));
    const download = page.waitForEvent('download'); await exporting.click(); expect((await download).suggestedFilename()).toMatch(/\.json$/);
    expect(server.sends).toHaveLength(1); expect(server.keeps).toHaveLength(0); await photograph(page, info, 'ask-running');
    await server.finish(); await expect(keeping).not.toHaveAttribute('aria-disabled', 'true');
    server.record(ASK_ID).conversation.can = { ...askConversation().can, message: true, export: false, keep: false };
    await server.finish();
    await reason(page, exporting, t('ask.actions.exportDenied')); await reason(page, keeping, t('ask.actions.keepDenied'));
    await keeping.click({ force: true }); expect(server.keeps).toHaveLength(0); await photograph(page, info, 'ask-refused');
  });
}
