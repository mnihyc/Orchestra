/** Provider settings reuse admitted names and keep unopened standalone names opaque until deliberate browsing. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, DOUBLE, PEOPLE, scenario } from './support.ts';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const key = 'agent.default_provider_id';
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`provider default reuses admitted names and defers unopened lookups ${width}/${theme}/${locale}`, async ({ page, request }) => {
    await scenario();
    const actor = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: PEOPLE.ada });
    expect(actor.ok()).toBe(true); const login: unknown = await actor.json();
    if (!record(login) || typeof login.access_token !== 'string') throw new Error('The settings fixture needs its fake sign-in result.');
    const written = await request.put(`${DOUBLE}/api/v1/nodes/${ACME.fixLogin}/settings/${key}`, {
      headers: { Authorization: `Bearer ${login.access_token}` }, data: { value: RUNS_ON.openAiKey },
    });
    expect(written.ok()).toBe(true);
    const current = await request.get(`${DOUBLE}/api/v1/providers/${RUNS_ON.openAiKey}`, { headers: { Authorization: `Bearer ${login.access_token}` } });
    expect(current.ok()).toBe(true); const provider: unknown = await current.json();
    if (!record(provider) || typeof provider.name !== 'string') throw new Error('The native fixture provider must have its authorized name');
    await signInScreenRoute(page, { width, theme, locale });
    let suppliedName: string | undefined;
    await page.route(`**/api/v1/nodes/${ACME.fixLogin}/chat-opening?*`, async (route) => {
      const response = await route.fetch(); expect(response.ok()).toBe(true);
      const body: unknown = await response.json();
      if (new URL(route.request().url()).searchParams.get('default_provider_name') === 'true') {
        if (!record(body) || !record(body.defaults) || !Array.isArray(body.defaults.providers)) throw new Error('The Defaults opening must supply its protected name member.');
        const entry: unknown = body.defaults.providers.find((value: unknown) => record(value) && value.setting === key && value.id === RUNS_ON.openAiKey);
        if (!record(entry) || entry.problem !== null || !record(entry.provider) || typeof entry.provider.name !== 'string') throw new Error('The current provider name must be independently admitted.');
        suppliedName = entry.provider.name;
      }
      // Observe the original native projection without replacing its authority or data.
      await route.fulfill({ response, json: body });
    });
    const providerPath = `/api/v1/providers/${RUNS_ON.openAiKey}`;
    const settingPath = `/api/v1/nodes/${ACME.fixLogin}/settings/${key}`;
    const reads: string[] = [], writes: string[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (request.method() === 'GET' && (url.pathname === providerPath || url.pathname === '/api/v1/everything' && url.searchParams.get('resource_view') === 'usable')) reads.push(url.pathname);
      if (['PUT', 'DELETE'].includes(request.method()) && url.pathname === settingPath) writes.push(request.method());
    });
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults`);
    const row = page.locator(`[data-setting="${key}"]`);
    const field = row.getByRole('combobox', { name: caption('@settingRegistry.agent.default_provider_id.title', locale), exact: true });
    await expect(field).toBeVisible();
    if (width === 400) {
      // The phone's closed Chat has no admitted Defaults opening or protected name slot.
      await expect(field).toHaveValue(caption('@places.providerDefault.selected', locale));
      expect(suppliedName).toBeUndefined();
    } else {
      await expect.poll(() => suppliedName).toBeDefined();
      await expect(field).toHaveValue(provider.name); expect(suppliedName).toBe(provider.name);
    }
    await expect(row).not.toContainText(RUNS_ON.openAiKey);
    await page.waitForLoadState('networkidle'); expect(reads).toEqual([]); expect(writes).toEqual([]);
    await field.click(); await expect(page.getByRole('dialog', { name: caption('@settingRegistry.agent.default_provider_id.title', locale), exact: true }).getByRole('option').first()).toBeVisible();
    await page.keyboard.press('Escape'); await expect(field).toHaveValue(provider.name);
    await expect(field).toHaveAttribute('aria-expanded', 'false');
    await page.waitForLoadState('networkidle');
    expect(reads.toSorted()).toEqual((width === 400 ? [providerPath, '/api/v1/everything'] : ['/api/v1/everything']).toSorted());
    expect(writes).toEqual([]);
  });
}
