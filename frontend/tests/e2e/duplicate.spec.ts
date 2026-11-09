/** Own click paths exercise the shared copy entrance in both languages, widths and themes. */
import { expect, test, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import type { CopyDestinationsRequest } from '../../src/api/copyDestinations';
import type { EverythingRowPage, Provider } from '../../src/generated/api-types';
import { scenario } from './support';

const COPY = '80000000-0000-4000-8000-000000000001';

for (const mode of SCREEN_PRESENTATIONS) test(`copy setup is visible before quick copy and on the native result (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Providers → row menu → credentials stay behind → Duplicate → accepted setup fact in the result inspector.
  // 中文：供应方 → 行菜单 → 查看凭据不随副本复制 → 创建副本 → 在结果详情中查看原生设置结果。
  const words = (key: string) => caption(`@${key}`, mode.locale);
  await scenario(); await resourceFixtures(page); await signInScreenRoute(page, mode);
  let source: Provider | null = null;
  await page.route('**/api/v1/everything?*', async (route) => {
    const response = await route.fetch();
    const result = await response.json() as EverythingRowPage;
    await route.fulfill({ response, json: { ...result, items: result.items.map((row) => {
      if (row.resource?.kind !== 'provider') return row;
      const provider: Provider = { ...row.resource.value, kind: 'api', access: { ...row.resource.value.access, permissions: row.resource.value.access.permissions.filter((permission) => permission !== 'access.own') } };
      source ??= provider;
      return { ...row, resource: { kind: 'provider', value: provider } };
    }) } });
  });
  await page.route(`**/api/v1/providers/${COPY}`, async (route) => {
    if (source === null) throw new Error('The copy fixture needs its actual readable source');
    await route.fulfill({ status: 200, json: { ...source, id: COPY, name: `${source.name} copy`, key_set: false, status: 'needs_setup', access: { ...source.access, permissions: [...source.access.permissions, 'provider.manage', 'access.own'] } } });
  });
  await page.route(/\/api\/v1\/providers\/[^/]+\/duplicate$/u, async (route) => {
    await route.fulfill({ status: 200, json: { projection: 'receipt', kind: 'provider', id: COPY, setup: ['credentials_not_copied'] } });
  });
  const secrets: string[] = [];
  page.on('request', (request) => { if (request.method() === 'GET' && /\/secrets(?:\/|$)/u.test(new URL(request.url()).pathname)) secrets.push(request.url()); });
  await page.goto('/resources/providers'); await screenReady(page, mode.locale, mode.theme);
  const row = page.getByRole('table', { name: words('providers.list.label'), exact: true }).getByRole('row').filter({ has: page.locator('a[href^="/resources/providers/"]') }).first();
  const name = await row.locator('a[href^="/resources/providers/"]').first().innerText();
  await row.getByRole('button', { name: words('resources.act.menuOf').replace('{{name}}', name), exact: true }).click();
  const action = page.getByRole('menuitem', { name: words('duplicate.now'), exact: true });
  await expect(action).toContainText(words('duplicate.setup.before.credentials_not_copied'));
  await action.click();
  await expect(page).toHaveURL(new RegExp(`/resources/providers/${COPY}`));
  await expect(page.locator('[data-copy-setup]').filter({ hasText: words('duplicate.setup.after.credentials_not_copied') })).toBeVisible();
  expect(secrets).toEqual([]); await bounded(page);
  await page.screenshot({ path: info.outputPath('copied-setup.png'), animations: 'disabled' });
});
async function destinationFixtures(page: Page) {
  await page.route(/\/api\/v1\/copy-destinations\/judge$/u, async (route) => {
    const body = route.request().postDataJSON() as CopyDestinationsRequest;
    await route.fulfill({ status: 200, json: { subject: body.subject, intent: body.intent, source_owned: false,
      destinations: body.destinations.map((destination) => ({ destination, cross_root: body.intent === 'move', outcome: body.intent === 'move'
        ? { state: 'blocked', cause: 'owners_only', problem: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Denied', detail: '', request_id: 'judge' } } : body.subject.kind === 'environment' ? { state: 'needs_choice', choice: 'environment_placement' } : { state: 'allowed' } })) } });
  });
}
async function bounded(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  for (const dialog of await page.getByRole('dialog').all()) {
    const box = await dialog.boundingBox(); expect(box).not.toBeNull();
    if (box !== null) { expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual((page.viewportSize()?.width ?? 0) + 1); }
  }
}
for (const mode of SCREEN_PRESENTATIONS) {
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  for (const surface of ['row', 'context', 'page'] as const) test(`native Duplicate from ${surface} (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Resources > Providers > row/menu or right-click > Duplicate > actual receipt identity opens
    // 中文：资源 > 供应方 > 行菜单或右键菜单 > 创建副本 > 打开回执指向的新资源
    await scenario(); await resourceFixtures(page); await destinationFixtures(page); await signInScreenRoute(page, mode);
    const writes: { path: string; body: unknown; key: string | undefined }[] = [];
    await page.route(/\/api\/v1\/providers\/[^/]+\/duplicate$/u, async (route) => {
      writes.push({ path: new URL(route.request().url()).pathname, body: route.request().postDataJSON(), key: route.request().headers()['idempotency-key'] });
      await route.fulfill({ status: 200, json: { projection: 'receipt', kind: 'provider', id: COPY } });
    });
    await page.goto('/resources/providers'); await screenReady(page, mode.locale, mode.theme);
    const table = page.getByRole('table', { name: words('providers.list.label'), exact: true });
    const row = table.getByRole('row').filter({ has: page.locator('a[href^="/resources/providers/"]') }).first();
    const link = row.locator('a[href^="/resources/providers/"]').first();
    const name = await link.innerText(); const href = await link.getAttribute('href'); expect(href).not.toBeNull();
    if (surface === 'page') {
      await page.goto(href!); await screenReady(page, mode.locale, mode.theme);
      await page.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).first().click();
    } else if (surface === 'context') await row.click({ button: 'right' });
    else await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click();
    const menu = page.getByRole('menu'); await expect(menu.getByRole('menuitem', { name: words('duplicate.to'), exact: true })).toBeVisible();
    await bounded(page); await page.screenshot({ path: info.outputPath(`${surface}-copy-menu.png`), animations: 'disabled' });
    await menu.getByRole('menuitem', { name: words('duplicate.now'), exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/resources/providers/${COPY}`));
    expect(writes).toHaveLength(1); expect(writes[0]?.key).toBeTruthy();
    expect(writes[0]?.body).toHaveProperty('name', null); expect(writes[0]?.body).toHaveProperty('destination', null); expect(writes[0]?.body).not.toHaveProperty('key');
  });
  test(`Duplicate to chooses a native place, cancels and keeps a refusal (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: Providers > Duplicate to > cancel > reopen > choose destination > native refusal > keep choice
    // 中文：供应方 > 创建副本到 > 取消 > 重新打开 > 选择位置 > 原生拒绝 > 保留选择
    await scenario(); await resourceFixtures(page); await destinationFixtures(page); await signInScreenRoute(page, mode);
    const writes: unknown[] = [];
    await page.route(/\/api\/v1\/providers\/[^/]+\/duplicate$/u, async (route) => {
      writes.push(route.request().postDataJSON());
      await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', status: 403, code: 'authz.denied', title: 'Denied', detail: '', request_id: 'copy-refused' } });
    });
    await page.goto('/resources/providers'); await screenReady(page, mode.locale, mode.theme);
    const table = page.getByRole('table', { name: words('providers.list.label'), exact: true });
    const row = table.getByRole('row').filter({ has: page.locator('a[href^="/resources/providers/"]') }).first();
    const name = await row.locator('a[href^="/resources/providers/"]').first().innerText();
    const open = async () => {
      await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click();
      await page.getByRole('menuitem', { name: words('duplicate.to'), exact: true }).click();
      return page.getByRole('dialog', { name: words('duplicate.title', { name }), exact: true });
    };
    let dialog = await open(); await dialog.getByRole('button', { name: words('common.cancel'), exact: true }).click(); expect(writes).toEqual([]);
    dialog = await open(); await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
    const allowed = page.locator('[role="option"]:not([aria-disabled="true"])').first();
    await expect(allowed).toBeVisible(); const selected = await allowed.getAttribute('aria-label') ?? (await allowed.innerText()).split('\n')[0] ?? ''; await allowed.click();
    await bounded(page); await page.screenshot({ path: info.outputPath('copy-to-place.png'), animations: 'disabled' });
    await dialog.getByRole('button', { name: words('duplicate.now'), exact: true }).click();
    await expect(dialog.getByRole('alert')).toBeVisible(); expect(writes).toHaveLength(1);
    expect(writes[0]).toMatchObject({ destination: { kind: 'node', id: expect.any(String) } });
    await expect(dialog.getByRole('button', { name: words('settings.place.clear', { path: selected }), exact: true })).toBeVisible();
    await bounded(page); await page.screenshot({ path: info.outputPath('native-copy-refusal.png'), animations: 'disabled' });
  });
}

for (const mode of SCREEN_PRESENTATIONS) test(`Move retains its resource identity inside one root (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Providers > Move > see other roots unavailable > choose home inside this root > native save
  // 中文：供应方 > 移动 > 查看其他组织不可用的原因 > 选择本组织内的位置 > 原生保存
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  await scenario(); await resourceFixtures(page); await destinationFixtures(page); await signInScreenRoute(page, mode);
  const writes: { path: string; body: unknown; key: string | undefined }[] = [];
  await page.route(/\/api\/v1\/providers\/[^/]+\/home$/u, async (route) => {
    const path = new URL(route.request().url()).pathname;
    const body: unknown = route.request().postDataJSON();
    writes.push({ path, body, key: route.request().headers()['idempotency-key'] });
    if (typeof body !== 'object' || body === null || !('home_id' in body) || typeof body.home_id !== 'string') throw new Error('The home move omitted its native destination');
    await route.fulfill({ status: 200, json: { projection: 'receipt', object_id: path.split('/').at(-2), kind: 'provider', home_id: body.home_id, changed: true } });
  });
  await page.goto('/resources/providers'); await screenReady(page, mode.locale, mode.theme);
  const row = page.getByRole('table', { name: words('providers.list.label'), exact: true }).getByRole('row').filter({ has: page.locator('a[href^="/resources/providers/"]') }).first();
  const name = await row.locator('a[href^="/resources/providers/"]').first().innerText();
  await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click();
  await page.getByRole('menuitem', { name: words('duplicate.move'), exact: true }).click();
  const dialog = page.getByRole('dialog', { name: words('duplicate.moveTitle', { name }), exact: true });
  await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
  await expect(page.locator('[role="option"]:not([aria-disabled="true"])').first()).toBeVisible();
  const refused = page.locator('[role="option"][aria-disabled="true"]').filter({ hasText: words('duplicate.causes.owners_only') });
  await expect(refused.first()).toBeVisible(); await bounded(page); await page.screenshot({ path: info.outputPath('native-root-choices.png'), animations: 'disabled' });
  await page.locator('[role="option"]:not([aria-disabled="true"])').first().click();
  await dialog.getByRole('button', { name: words('duplicate.moveConfirm'), exact: true }).click();
  await expect(dialog).toHaveCount(0); expect(writes).toHaveLength(1); expect(writes[0]?.key).toBeTruthy();
  expect(writes[0]?.body).toMatchObject({ home_id: expect.any(String) });
  expect(writes[0]?.body).not.toHaveProperty('destination'); expect(writes[0]?.body).not.toHaveProperty('owner');
  await expect(page).toHaveURL(/\/resources\/providers$/u);
});

for (const mode of SCREEN_PRESENTATIONS) test(`Environment Duplicate to reviews placement (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Environments > Duplicate to > destination > Managed environment > actual native copy acknowledgement
  // 中文：环境 > 创建副本到 > 目标位置 > 托管环境 > 接受原生创建副本回执
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  await scenario(); await resourceFixtures(page); await destinationFixtures(page); await signInScreenRoute(page, mode);
  const writes: unknown[] = [];
  await page.route(/\/api\/v1\/environments\/[^/]+\/duplicate$/u, async (route) => {
    writes.push(route.request().postDataJSON()); await route.fulfill({ status: 200, json: { projection: 'receipt', kind: 'environment', id: COPY } });
  });
  await page.goto('/resources/environments'); await screenReady(page, mode.locale, mode.theme);
  const row = page.getByRole('table').getByRole('row').filter({ has: page.locator('a[href^="/resources/environments/"]') }).first();
  const name = await row.locator('a[href^="/resources/environments/"]').first().innerText();
  await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click(); await page.getByRole('menuitem', { name: words('duplicate.to'), exact: true }).click();
  const dialog = page.getByRole('dialog', { name: words('duplicate.title', { name }), exact: true });
  await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
  await page.locator('[role="option"]:not([aria-disabled="true"])').first().click();
  await dialog.getByRole('combobox', { name: words('duplicate.placement'), exact: true }).click();
  await page.getByRole('option', { name: words('duplicate.managedPlacement'), exact: true }).click();
  await bounded(page); await page.screenshot({ path: info.outputPath('environment-copy-placement.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: words('duplicate.now'), exact: true }).click(); await expect(page).toHaveURL(new RegExp(`/resources/environments/${COPY}`));
  expect(writes).toHaveLength(1); expect(writes[0]).toMatchObject({ managed: { template_id: null, host_id: null } });
});
