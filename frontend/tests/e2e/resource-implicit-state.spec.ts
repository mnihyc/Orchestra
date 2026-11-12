/** Own bilingual click paths for AI-12 management refusals and AI-13 sharing read states. Authored for integrator execution. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { RUNS_ON } from '../doubles/fixtures';
import { scenario } from './support';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function viewOnly(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(viewOnly);
  if (!record(value)) return value;
  const next: Record<string, unknown> = Object.fromEntries(Object.entries(value).map(([key, item]) => [key, viewOnly(item)]));
  if (record(next.access) && Array.isArray(next.access.permissions)) next.access = { ...next.access, administers: false, admin_only: [], admin_access: null, permissions: next.access.permissions.filter((permission: unknown) => typeof permission === 'string' && !permission.endsWith('.manage')) };
  return next;
}
for (const mode of SCREEN_PRESENTATIONS) {
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  test(`AI-12 readable profile > refused row acts > refused page editor (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Resources > Profiles > row Actions > disabled Rename > Open > disabled Edit > keyboard reason; no write
    // 中文：资源 > 配置 > 行操作 > 不可用的重命名 > 打开 > 不可用的编辑 > 键盘查看原因；没有写入
    await scenario(); let writes = 0;
    // The library's native inventory supplies row authority; the detail reader supplies the page authority.
    await page.route(/\/api\/v1\/(?:agents(?:\/[^/?]+)?|everything)(?:\?|$)/u, async (route) => {
      if (route.request().method() !== 'GET') { writes += 1; await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Denied', detail: '', request_id: 'read-only-profile' } }); return; }
      const response = await route.fetch(); const body: unknown = await response.json(); await route.fulfill({ response, json: viewOnly(body) });
    });
    await signInScreenRoute(page, mode);
    // Live resource snapshots must have the same ordinary-reader authority as the intercepted HTTP reads.
    const administration = page.locator('[data-screen-titlebar]:visible [data-as-admin]');
    if (await administration.getAttribute('data-as-admin') === 'true') await administration.click();
    await expect(administration).toHaveAttribute('data-as-admin', 'false');
    await page.goto('/resources/agents'); await screenReady(page, mode.locale, mode.theme);
    const link = page.locator(`a[href="/resources/agents/${RUNS_ON.codex}"]`).first();
    const name = await link.innerText(); const row = page.getByRole('row').filter({ has: page.locator(`a[href="/resources/agents/${RUNS_ON.codex}"]`) }).first();
    await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click();
    const rename = page.getByRole('menuitem', { name: new RegExp(words('resources.act.rename')) });
    await expect(rename).toHaveAttribute('aria-disabled', 'true'); await expect(rename).toContainText(words('resources.act.manageRequired'));
    await rename.press('Enter'); await expect(page.getByRole('dialog')).toHaveCount(0); await page.keyboard.press('Escape');
    await link.click(); await expect(page).toHaveURL(new RegExp(`/resources/agents/${RUNS_ON.codex}`)); await screenReady(page, mode.locale, mode.theme);
    const edit = page.getByRole('button', { name: words('resources.act.editButton'), exact: true }).first();
    await expect(edit).toHaveAttribute('aria-disabled', 'true'); await edit.focus(); await edit.press('Enter');
    await expect(page.getByRole('dialog')).toHaveCount(0); expect(writes).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('read-only-management.png'), animations: 'disabled' });
  });
  test(`AI-13 profile card > checking > failed read > Retry > actual Share (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Resources > Profiles > focus name card > Checking sharing > unavailable > Retry > Share > Escape
    // 中文：资源 > 配置 > 检查卡片 > 正在检查共享 > 详情不可用 > 重试 > 共享 > Escape
    await scenario(); let phase: 'waiting' | 'failed' | 'ready' = 'waiting', reads = 0;
    let release: (() => void) | undefined; const gate = new Promise<void>((resolve) => { release = resolve; });
    await page.route(new RegExp(`/api/v1/agents/${RUNS_ON.codex}/permissions(?:\\?|$)`), async (route) => {
      reads += 1; if (phase === 'waiting') await gate;
      if (phase !== 'ready') { await route.fulfill({ status: 503, json: { type: 'orch:error:internal', code: 'internal', status: 503, title: 'Sharing read failed', detail: '', request_id: 'sharing-read' } }); return; }
      await route.continue();
    });
    await signInScreenRoute(page, mode); await page.goto('/resources/agents'); await screenReady(page, mode.locale, mode.theme);
    const link = page.locator(`a[href="/resources/agents/${RUNS_ON.codex}"]`).first();
    // Names expose their card on focus; the separate Inspect button belongs to picker choices.
    await link.focus();
    const card = page.locator('[data-resource-card="agent"]').last();
    await card.hover();
    await expect(card.getByText(words('linkCards.sharingChecking'), { exact: true })).toBeVisible();
    phase = 'failed'; release?.();
    await expect(card.getByText(words('linkCards.sharingUnavailable'), { exact: true })).toBeVisible();
    await expect(card.getByRole('button', { name: words('linkCards.share'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    phase = 'ready'; await card.getByRole('button', { name: words('common.tryAgain'), exact: true }).click();
    const share = card.getByRole('button', { name: words('linkCards.share'), exact: true });
    await expect(share).not.toHaveAttribute('aria-disabled', 'true'); await share.click();
    await expect(page.getByRole('dialog')).toBeVisible(); expect(reads).toBeGreaterThanOrEqual(2); await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('sharing-read-recovered.png'), animations: 'disabled' });
  });
}
