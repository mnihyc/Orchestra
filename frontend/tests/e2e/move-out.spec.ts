/** An actual menu destination requires a native delete footprint and one atomic write. */
import { expect, test } from '@playwright/test';
import type { CopyDestinationsRequest } from '../../src/api/copyDestinations';
import type { MoveOutBody } from '../../src/api/moveOut';
import { caption } from '../../e2e/capture/open';
import { prepareLayoutRoute, resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { scenario } from './support';
const COPY = '80000000-0000-4000-8000-000000000099';
for (const mode of SCREEN_PRESENTATIONS) for (const family of ['agents', 'providers'] as const) test(`native cross-org ${family} move (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Agents or Providers > Move > other org > native delete/stop plan > cancel > reopen > one copy-and-delete request
  // 中文：代理或供应方 > 移动 > 其他组织 > 原生删除及停止计划 > 取消 > 重新打开 > 一次复制并删除请求
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  await scenario(); await resourceFixtures(page); await prepareLayoutRoute(page, { pattern: `/resources/${family}`, url: `/resources/${family}` }); await signInScreenRoute(page, mode);
  const kind = family === 'agents' ? 'agent' : 'provider';
  const effects = ['remove_source', 'stop_bound_work'];
  let nativePlan: unknown; const writes: MoveOutBody[] = [], deletes: string[] = [];
  await page.route(/\/api\/v1\/copy-destinations\/judge$/u, async (route) => {
    const body = route.request().postDataJSON() as CopyDestinationsRequest;
    await route.fulfill({ status: 200, json: { subject: body.subject, intent: body.intent, source_owned: true, destinations: body.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'needs_choice', choice: 'delete_preview' } })) } });
  });
  await page.route(new RegExp(`/api/v1/${family}/([^/]+)/move-out(?:/preview)?$`), async (route) => {
    const source = new RegExp(`/${family}/([^/]+)/`).exec(new URL(route.request().url()).pathname)?.[1];
    if (source === undefined) throw new Error('The move route omitted its source');
    if (new URL(route.request().url()).pathname.endsWith('/preview')) {
      const body = route.request().postDataJSON() as MoveOutBody['copy'];
      nativePlan = { subject: { kind, id: source }, destination: body.destination, revision: body.expected_revision ?? 1, stamp: 'native-delete-footprint', effects };
      await route.fulfill({ status: 200, json: nativePlan }); return;
    }
    writes.push(route.request().postDataJSON() as MoveOutBody); await route.fulfill({ status: 200, json: { projection: 'receipt', kind, id: COPY } });
  });
  page.on('request', (request) => { if (request.method() === 'DELETE' && request.url().includes(`/api/v1/${family}/`)) deletes.push(request.url()); });
  await page.goto(`/resources/${family}`); await screenReady(page, mode.locale, mode.theme);
  const row = page.getByRole('table').getByRole('row').filter({ has: page.locator(`a[href^="/resources/${family}/"]`) }).first();
  const name = await row.locator(`a[href^="/resources/${family}/"]`).first().innerText();
  const open = async () => {
    await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click();
    await page.getByRole('menuitem', { name: words('duplicate.move'), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words('duplicate.moveTitle', { name }), exact: true });
    await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
    // Same-root candidates use the existing home act; the new contract judges other roots.
    const other = page.getByRole('option').filter({ hasText: words('duplicate.choices.delete_preview') }).first();
    await expect(other).not.toHaveAttribute('aria-disabled', 'true'); const destination = (await other.innerText()).split('\n')[0] ?? ''; await other.click();
    await expect(dialog.getByRole('table', { name: words('duplicate.deletePlan'), exact: true })).toBeVisible(); return { dialog, destination };
  };
  let { dialog } = await open(); await dialog.getByRole('button', { name: words('common.cancel'), exact: true }).click(); expect(writes).toEqual([]);
  ({ dialog } = await open());
  await expect(dialog.getByText(words('duplicate.moveEffects.stop_bound_work'), { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath('move-native-delete-plan.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: new RegExp(mode.locale === 'en' ? '^Copy to ' : '^复制到') }).click();
  await expect(page).toHaveURL(new RegExp(`/resources/${family}/${COPY}`)); expect(writes).toHaveLength(1); expect(deletes).toEqual([]);
  expect(writes[0]?.delete_plan).toMatchObject({ stamp: 'native-delete-footprint' }); expect(nativePlan).toBeDefined();
});
