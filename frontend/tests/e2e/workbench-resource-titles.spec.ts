/** Real hard reloads retain public resource-tab identity without admitting inactive list readers. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';

const resources = [
  ['providers', 'provider', 'Providers', '提供方'], ['agents', 'agent', 'Agents', '智能体'],
  ['machines', 'machine', 'Machines', '机器'], ['environments', 'environment', 'Environments', '环境'],
  ['templates', 'template', 'Templates', '模板'], ['hosts', 'host', 'Hosts', '主机'],
  ['mcp', 'mcp_server', 'MCP servers', 'MCP 服务器'], ['mounts', 'mount', 'Mounts', '挂载'],
] as const;

for (const locale of ['en', 'zh-CN'] as const) test(`resource tabs retain names, identities and quiet inactive readers through hard reloads in ${locale}`, async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await scenario(); await signIn(page);
  await expect(page.locator('[data-main-area]').getByRole('heading', { level: 1 }).first()).toBeVisible();
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: 'Language', exact: true }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
  }
  const main = page.locator('[data-main-area]'), saved = new Map<string, string>();
  const reads: string[] = [], faults: string[] = [];
  page.on('pageerror', (error) => faults.push(error.message));
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() === 'GET' && url.pathname === '/api/v1/everything' && url.searchParams.get('resource_details') === 'true') reads.push(url.searchParams.get('kinds') ?? '');
  });
  for (const [kind, nativeKind, english, chinese] of resources) {
    const title = locale === 'en' ? english : chinese;
    reads.length = 0;
    await page.goto(`/resources/${kind}`);
    await expect(main.getByRole('heading', { name: title, exact: true })).toBeVisible();
    await expect.poll(() => reads).toEqual([nativeKind]);
    const tab = main.getByRole('tab', { name: title, exact: true });
    await expect(tab).toHaveAttribute('aria-selected', 'true');
    const identity = await tab.getAttribute('id');
    if (identity === null) throw new Error('The resource tab lost its identity');
    saved.set(title, identity);
    for (const [name, id] of saved) await expect(main.getByRole('tab', { name, exact: true })).toHaveAttribute('id', id);
  }
  reads.length = 0;
  await page.reload();
  await expect(main.getByRole('heading', { name: locale === 'en' ? 'Mounts' : '挂载', exact: true })).toBeVisible();
  await expect.poll(() => reads).toEqual(['mount']);
  const selected = main.getByRole('tab', { name: locale === 'en' ? 'Mounts' : '挂载', exact: true });
  await expect(selected).toBeInViewport({ ratio: 1 });
  await expect.poll(() => selected.evaluate((tab) => {
    const strip = tab.closest('[role="tablist"]');
    if (!(strip instanceof HTMLElement)) throw new Error('The selected tab must belong to its real strip');
    let whole = tab;
    while (whole.parentElement !== null && whole.parentElement !== strip) whole = whole.parentElement;
    const close = whole.querySelector('button');
    if (close === null) throw new Error('The normal resource tab must expose its close control');
    const viewport = strip.getBoundingClientRect(), bounds = close.getBoundingClientRect();
    return bounds.left >= viewport.left + strip.clientLeft && bounds.right <= viewport.left + strip.clientLeft + strip.clientWidth;
  })).toBe(true);
  for (const [name, id] of saved) await expect(main.getByRole('tab', { name, exact: true })).toHaveAttribute('id', id);
  await expect(page).toHaveURL(/\/resources\/mounts\/?$/);
  expect(faults).toEqual([]);
  await page.screenshot({ path: info.outputPath(`resource-titles-${locale}.png`), animations: 'disabled' });
  reads.length = 0;
  await selected.locator('..').locator('button').click();
  await expect(selected).toHaveCount(0);
  await expect(page).toHaveURL(/\/resources\/mcp\/?$/);
  await expect.poll(() => reads).toEqual(['mcp_server']);
});
