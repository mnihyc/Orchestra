/** Own bilingual click paths for System usage and management-visible directory inventory. */
import { expect, test } from '@playwright/test';
import type { Machine, MachineDataInventory, UsageAnalytics, UsageDimension } from '../../src/generated/api-types';
import { caption } from '../../e2e/capture/open';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { requestRow } from '../doubles/request-row';
import { scenario } from './support';
import { RUNS_ON } from '../doubles/fixtures';
for (const mode of SCREEN_PRESENTATIONS) {
  const words = (key: string) => caption(`@${key}`, mode.locale);
  test(`System usage survives the address and uses its native request feed (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Usage > Scope System > reload > Performance > Requests > live System feed > Me
    // 中文：用量 > 范围系统 > 刷新 > 性能 > 请求 > 系统实时记录 > 我
    await scenario(); const scopes: string[] = [], feeds: string[] = [];
    const nativeRow = requestRow({ at: new Date().toISOString(), model_display: 'System request model' });
    await page.route(/\/api\/v1\/usage(?:\?|$)/u, async (route) => {
      const query = new URL(route.request().url()).searchParams, scope = query.get('scope') ?? 'me'; scopes.push(scope);
      const totals = { turns: scope === 'system' ? 7 : 1, tokens: { input_tokens: 10, cached_input_tokens: 0, output_tokens: 5, reasoning_tokens: 0, total_tokens: 15 }, credits: '1', estimated_credits: '0', unrated_tokens: 0 };
      const data: UsageAnalytics = { scope, depth: scope === 'system' ? 'reports' : 'own', from: query.get('from') ?? '', to: query.get('to') ?? '', bucket: 'day', totals, me: null, series: [], breakdowns: (query.get('breakdowns')?.split(',').filter(Boolean) ?? []).map((by) => ({ by: by as UsageDimension, more: 0, rows: [] })) };
      await route.fulfill({ json: data });
    });
    await page.route(/\/api\/v1\/usage\/events(?:\?|$)/u, async (route) => { const scope = new URL(route.request().url()).searchParams.get('scope'); if (scope !== null) scopes.push(scope); await route.fulfill({ json: { items: scope === 'system' ? [nativeRow] : [], next_cursor: null } }); });
    await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/u, (socket) => {
      const server = socket.connectToServer(); const held = new Set<number>();
      socket.onMessage((raw) => { const message = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
        if (message.op === 'sub' && message.id !== undefined && message.feed?.startsWith('usage:system') === true) { held.add(message.id); feeds.push(message.feed);
          socket.send(JSON.stringify({ op: 'subscribed', id: message.id, feed: message.feed, seq: null, mode: 'ephemeral' })); socket.send(JSON.stringify({ op: 'event', id: message.id, feed: message.feed, data: { items: [nativeRow], next_cursor: null } }));
        } else if (message.op === 'unsub' && message.id !== undefined && held.has(message.id)) held.delete(message.id); else server.send(raw);
      });
      server.onMessage((raw) => socket.send(raw));
    });
    await signInScreenRoute(page, mode); await page.goto('/usage'); await screenReady(page, mode.locale, mode.theme);
    await page.getByRole('combobox', { name: words('usage.scope.label'), exact: true }).click(); await page.getByRole('option', { name: words('usage.scope.system'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('scope')).toBe('system'); await page.reload(); await screenReady(page, mode.locale, mode.theme);
    await expect(page.getByRole('combobox', { name: words('usage.scope.label'), exact: true })).toHaveValue(words('usage.scope.system'));
    await page.getByRole('tab', { name: words('usagePerf.viewNames.performance'), exact: true }).click(); await expect.poll(() => scopes.includes('system')).toBe(true);
    await page.getByRole('tab', { name: words('usagePerf.viewNames.requests'), exact: true }).click(); await expect.poll(() => feeds.includes('usage:system')).toBe(true); await expect(page.getByText('System request model', { exact: true }).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true); await page.mouse.move(0, 0); await page.screenshot({ path: info.outputPath('system-usage-requests.png'), animations: 'disabled' });
    await page.getByRole('combobox', { name: words('usage.scope.label'), exact: true }).click(); await page.getByRole('option', { name: words('usage.scope.me.name'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('scope')).not.toBe('system'); await expect(page.getByText('System request model', { exact: true })).toHaveCount(0);
  });
  test(`Machine Storage shows measured metadata and a sealed name-size row (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Machines > machine > Storage > directory observation > incomplete/unlisted > sealed store with no Open
    // 中文：机器 > 机器详情 > 存储 > 目录观测 > 不完整及未列出项 > 密封存储无打开入口
    await scenario(); const observed = '2026-10-15T08:12:00Z'; const inventory: MachineDataInventory = { observed_at: observed, complete: false, unlisted_parts: 3, problem: null, parts: [
      { id: 'env-meta', name: 'environments', kind: 'environment_data', bytes: 4096, problem: null }, { id: 'logs-meta', name: 'logs', kind: 'logs', bytes: null, problem: 'unreadable' }, { id: 'sealed-not-a-file-address', name: 'credentials.db', kind: 'sealed_store', bytes: 128, problem: null },
    ] };
    await page.route(new RegExp(`/api/v1/machines/${RUNS_ON.laptop}(?:\\?|$)`), async (route) => { const response = await route.fetch(); const machine = await response.json() as Machine; await route.fulfill({ response, json: { ...machine, data_directory: inventory } }); });
    await signInScreenRoute(page, mode); await page.goto(`/resources/machines/${RUNS_ON.laptop}`); await screenReady(page, mode.locale, mode.theme);
    await page.getByRole('tab', { name: words('resourceDetails.layout.storage'), exact: true }).click();
    const sections = page.getByRole('navigation', { name: words('resourceDetails.layout.storage'), exact: true });
    const storageLink = sections.getByRole('link', { name: words('machineStorage.title'), exact: true });
    if (await storageLink.isVisible()) await storageLink.click(); else { await sections.getByRole('combobox').click(); await page.getByRole('option', { name: words('machineStorage.title'), exact: true }).click(); }
    const parts = page.getByRole('table', { name: words('machineData.parts'), exact: true }); await expect(parts.getByText('environments', { exact: true })).toBeVisible();
    await expect(page.locator(`time[datetime="${observed}"]`)).toBeVisible(); await expect(page.getByText(new RegExp(words('machineData.incomplete')))).toBeVisible();
    const sealed = page.getByRole('table', { name: words('machineData.sealed'), exact: true }); await expect(sealed.getByRole('columnheader')).toHaveCount(2); await expect(sealed.getByText('credentials.db', { exact: true })).toBeVisible();
    await expect(sealed.getByRole('link')).toHaveCount(0); await expect(sealed.getByRole('button')).toHaveCount(0); await expect(sealed).not.toContainText('sealed-not-a-file-address');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true); await page.mouse.move(0, 0); await page.screenshot({ path: info.outputPath('machine-directory-inventory.png'), animations: 'disabled' });
  });
}
