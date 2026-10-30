/** Shipping frame traffic and retained inventory data on a hard opening and an in-app return. */
import { expect, test, type Request } from '@playwright/test';
import { resourceFixtures } from '../../e2e/layout-fixtures.ts';
import { scenario, signIn } from '../e2e/support.ts';

const kinds = ['providers', 'agents', 'environments', 'hosts', 'templates', 'mcp', 'mounts', 'machines', 'skills'] as const;
function record(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }
function namesOf(value: unknown): string[] {
  if (!record(value) || !Array.isArray(value.items)) throw new Error('The native inventory lost its page.');
  return value.items.flatMap((item: unknown) => {
    if (!record(item)) throw new Error('An inventory row lost its native identity.');
    const resource = record(item.resource) ? item.resource : null;
    const resourceValue = resource !== null && record(resource.value) ? resource.value : null;
    const name = typeof item.name === 'string' ? item.name : resourceValue?.name;
    return typeof name === 'string' && name !== '' ? [name] : [];
  });
}
for (const width of [1440, 400]) for (const kind of kinds) {
  test(`frame ${kind} cold and warm at ${width}`, async ({ page }, info) => {
    await scenario();
    await resourceFixtures(page);
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: 'light' });
    await signIn(page);
    const reads: string[] = [];
    const listen = (request: Request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') reads.push(`${request.method()} ${url.pathname}${url.search}`);
    };
    page.on('request', listen);
    const path = `/resources/${kind}`;
    let cold: string[] = [], warm: string[] = [];
    let inventory: unknown = null;
    let coldComplete = false, warmComplete = false;
    try {
      const own = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname === '/api/v1/everything');
      const checked = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/me/authority');
      await page.goto(path);
      const response = await own;
      expect(response.status()).toBe(200);
      expect((await checked).status()).toBe(200);
      inventory = await response.json();
      cold = [...reads];
      const identities = namesOf(inventory);
      expect(identities.length, 'The fixture must retain visible inventory records').toBeGreaterThan(0);
      const strip = page.locator(width === 400 ? '[data-mobile-main-tabs]' : '[data-main-area] [data-navigation-strip="views"]');
      const selected = strip.getByRole('tab', { selected: true });
      await expect(selected).toBeVisible();
      const tab = await selected.getAttribute('id'), panel = await selected.getAttribute('aria-controls');
      if (tab === null || panel === null) throw new Error('The inventory lost its native tab and panel.');
      const body = page.locator(`[id=${JSON.stringify(panel)}]`);
      for (const name of new Set(identities)) await expect(body.getByText(name, { exact: true }).first()).toBeVisible();
      if (width === 1440) await expect(page.getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
      await page.waitForLoadState('networkidle');
      cold = [...reads];
      coldComplete = true;
      await info.attach('frame-cold', { body: await page.screenshot(), contentType: 'image/png' });
      const documentTime = await page.evaluate(() => performance.timeOrigin);
      await page.getByRole('button', { name: 'Your account', exact: true }).click();
      await page.getByRole('menuitem', { name: 'Settings', exact: true }).click();
      await expect(page).toHaveURL(/\/settings\/profile(?:\?|$)/);
      await page.waitForLoadState('networkidle');
      reads.length = 0;
      if (width === 400) {
        await page.getByRole('button', { name: 'Find tab in Main area', exact: true }).click();
        await page.locator(`[role="option"][data-value=${JSON.stringify('commands:workspace.focus.' + tab.slice('mobile-tab-'.length))}]`).click();
      } else await page.locator(`[role="tab"][id=${JSON.stringify(tab)}]`).click();
      await expect(strip.getByRole('tab', { selected: true })).toHaveAttribute('id', tab);
      await expect(page).toHaveURL(new RegExp(`/resources/${kind}(?:\\?|$)`));
      for (const name of new Set(identities)) await expect(body.getByText(name, { exact: true }).first()).toBeVisible();
      await page.waitForLoadState('networkidle');
      expect(await page.evaluate(() => performance.timeOrigin), 'The warm return must reuse the actual document and caches').toBe(documentTime);
      warm = [...reads];
      warmComplete = true;
      await info.attach('frame-warm', { body: await page.screenshot(), contentType: 'image/png' });
      expect(cold.length, cold.join('\n')).toBeLessThanOrEqual(8);
      expect(cold.filter((read) => read === 'GET /api/v1/meta')).toHaveLength(1);
      expect(cold.filter((read) => read === 'GET /api/v1/me/bootstrap')).toHaveLength(1);
      expect(cold.filter((read) => read === 'GET /api/v1/me/authority')).toHaveLength(1);
      expect(cold.filter((read) => /^GET \/api\/v1\/(?:me|me\/admin-mode|orgs)$/.test(read))).toEqual([]);
      expect(cold.filter((read) => /^GET \/api\/v1\/nodes\/[^/]+\/documents\?/.test(read))).toEqual([]);
      expect(cold.filter((read) => /^GET \/api\/v1\/nodes\/[^/]+\/settings$/.test(read))).toEqual([]);
      const batches = cold.filter((read) => read.includes('/document-pages?'));
      expect(batches).toHaveLength(width === 1440 ? 1 : 0);
      if (width === 1440) expect(new URL(batches[0]!.slice(4), 'http://fixture.test').searchParams.get('node_ids')?.split(',')).toHaveLength(4);
      expect(warm.length, warm.join('\n')).toBeLessThanOrEqual(8);
    } finally {
      page.off('request', listen);
      await info.attach('frame-traffic', { body: JSON.stringify({ width, kind, cold, warm, coldComplete, warmComplete, observed: reads, inventory }), contentType: 'application/json' });
    }
  });
}
