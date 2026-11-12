/** Defaults metadata is optional at first paint and is read only through a deliberate picker or names action. */
import { expect, test, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, DOUBLE, PEOPLE, scenario } from './support.ts';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
/** Buffer each original GET through the routing API; Chromium may evict its own response body. */
async function captureRead(page: Page, path: string): Promise<{ body: Promise<unknown> }> {
  let received: (value: unknown) => void = () => undefined;
  const body = new Promise<unknown>((resolve) => { received = resolve; });
  await page.route(`**${path}`, async (route) => {
    const response = await route.fetch(); expect(response.ok()).toBe(true);
    const value: unknown = await response.json(); await route.fulfill({ response, json: value }); received(value);
  });
  return { body };
}
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`closed Defaults waits for its metadata reader ${width}/${theme}/${locale}`, async ({ page, request }) => {
    await scenario();
    const actor = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: PEOPLE.ada });
    expect(actor.ok()).toBe(true); const login: unknown = await actor.json();
    if (!record(login) || typeof login.access_token !== 'string') throw new Error('The fake settings author needs its native token.');
    const written = await request.put(`${DOUBLE}/api/v1/nodes/${ACME.fixLogin}/settings/agent.default_provider_id`, {
      headers: { Authorization: `Bearer ${login.access_token}` }, data: { value: RUNS_ON.openAiKey },
    });
    expect(written.ok()).toBe(true);
    await signInScreenRoute(page, { width, theme, locale });
    // A valid opening may omit optional metadata. Its readers must remain demand-driven in that fallback too.
    await page.route(`**/api/v1/nodes/${ACME.fixLogin}/chat-opening?*`, async (route) => {
      const response = await route.fetch(); const body: unknown = await response.json();
      if (!record(body) || !record(body.defaults)) throw new Error('The Defaults fixture needs its typed native opening.');
      body.defaults.providers = []; body.defaults.secrets = null;
      await route.fulfill({ response, json: body });
    });
    const providerPath = `/api/v1/providers/${RUNS_ON.openAiKey}`, secretPath = `/api/v1/nodes/${ACME.fixLogin}/secrets`;
    const reads: string[] = []; page.on('request', (request) => { const path = new URL(request.url()).pathname; if (request.method() === 'GET' && [providerPath, secretPath].includes(path)) reads.push(path); });
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults`);
    const provider = page.locator('[data-setting="agent.default_provider_id"]').getByRole('combobox');
    await expect(provider).toBeVisible(); await page.waitForLoadState('networkidle'); expect(reads).toEqual([]);
    const nameRead = await captureRead(page, providerPath);
    await provider.click(); const native: unknown = await nameRead.body; if (!record(native) || typeof native.name !== 'string') throw new Error('The current provider read must carry its authorized name.');
    await expect(page.getByRole('listbox').getByRole('option').first()).toBeVisible(); await page.keyboard.press('Escape');
    await expect(provider).toHaveValue(native.name); expect(reads).toEqual([providerPath]);
    const namesRead = await captureRead(page, secretPath);
    await page.getByRole('button', { name: caption('@settings.secrets.showNames', locale), exact: true }).click();
    const names: unknown = await namesRead.body; expect(Array.isArray(names)).toBe(true);
    if (!Array.isArray(names)) throw new Error('Only native secret metadata is expected.');
    expect(names.every((entry: unknown) => record(entry) && !('value' in entry))).toBe(true);
    await expect(page.getByRole('button', { name: caption('@settings.secrets.showNames', locale), exact: true })).toHaveCount(0);
    expect(reads).toEqual([providerPath, secretPath]);
  });
}
