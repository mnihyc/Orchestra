/** Production request counts for Defaults, resource changes, deliberate tree expansion and hidden team chat. */
import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';
import { ACME, PEOPLE, scenario, signIn } from '../e2e/support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';

const API = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/api/v1`;
const IDLE_MS = 60_000;
const FIRST_LOAD = 8;
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

function traffic(page: Page) {
  const requests: { method: string; url: URL }[] = [];
  const receive = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push({ method: request.method(), url });
  };
  page.on('request', receive);
  return { requests, lines: () => requests.map(({ method, url }) => `${method} ${url.pathname}${url.search}`), stop: () => page.off('request', receive) };
}

async function settled(page: Page) {
  await page.waitForLoadState('networkidle');
  expect(await page.locator('script[src]').evaluateAll((scripts) => scripts.some((script) => script.getAttribute('src')?.startsWith('/src/') === true)), 'Count the production application').toBe(false);
  await expect(page.locator('body')).not.toContainText('Unexpected Application Error');
}

async function actor(request: APIRequestContext): Promise<{ token: string; userId: string }> {
  const response = await request.post(`${API}/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
  expect(response.ok()).toBe(true);
  const body: unknown = await response.json();
  if (!record(body) || typeof body.access_token !== 'string' || !record(body.user) || typeof body.user.id !== 'string') throw new Error('The native fake sign-in must supply its reader');
  return { token: body.access_token, userId: body.user.id };
}

for (const path of ['/org/acme/settings/policies', `/project/${ACME.web}/settings?section=policies`, `/topic/${ACME.fixLogin}?tab=settings&section=policies`]) {
  test(`Defaults at ${path} reads no closed options and makes no request in an idle minute`, async ({ page }) => {
    await scenario(); await signIn(page);
    const measured = traffic(page);
    try {
      await page.goto(path);
      await expect(page.locator('[data-setting-row]').first()).toBeVisible();
      await settled(page);
      expect(measured.requests.length, measured.lines().join('\n')).toBeLessThanOrEqual(FIRST_LOAD);
      expect(measured.requests.filter(({ method, url }) => method === 'GET' && /^\/api\/v1\/(hosts|templates|providers)$/.test(url.pathname))).toEqual([]);
      const initial = measured.requests.length;
      await page.clock.install(); await page.clock.fastForward(IDLE_MS); await settled(page);
      expect(measured.requests.slice(initial), measured.lines().join('\n')).toEqual([]);
      const host = page.locator('[data-setting="env.host_id"], [data-setting="runtime.host_id"]').getByRole('combobox').first();
      await expect(host).toBeEnabled();
      const options = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname === '/api/v1/hosts');
      await host.click(); expect((await options).ok()).toBe(true);
      await expect(page.getByRole('option').first()).toBeVisible();
      expect(measured.requests.slice(initial).filter(({ method, url }) => method === 'GET' && url.pathname === '/api/v1/hosts')).toHaveLength(1);
      await page.keyboard.press('Escape');
    } finally { measured.stop(); }
  });
}

test('provider Settings consumes live native rows without reading the provider, hosts or their lists again', async ({ page, request }) => {
  await scenario(); await signIn(page);
  const other = await actor(request), headers = { Authorization: `Bearer ${other.token}` };
  const providerId = RUNS_ON.teamAnthropic;
  await page.goto(`/resources/providers/${providerId}?tab=settings`); await settled(page);
  const measured = traffic(page);
  try {
    const beforeResponse = await request.get(`${API}/providers/${providerId}`, { headers }); expect(beforeResponse.ok()).toBe(true);
    const before: unknown = await beforeResponse.json();
    if (!record(before) || typeof before.revision !== 'number') throw new Error('The provider must have its native revision');
    const changed = await request.patch(`${API}/providers/${providerId}`, { headers, data: { name: 'Provider changed on another screen', expected_revision: before.revision } });
    expect(changed.ok()).toBe(true);
    await expect(page.getByRole('heading', { level: 1, name: 'Provider changed on another screen', exact: true })).toBeVisible();
    await settled(page);
    expect(measured.requests, measured.lines().join('\n')).toEqual([]);
    await page.clock.install(); await page.clock.fastForward(IDLE_MS); await settled(page);
    expect(measured.requests, measured.lines().join('\n')).toEqual([]);
  } finally { measured.stop(); }
});

test('resource visits do not expand tree kinds, and a deliberate expansion reads exactly its own names', async ({ page, request }) => {
  await scenario(); await signIn(page); const other = await actor(request);
  // Old device data mixed explicit expansions with automatic reveals. It must not revive every visited kind.
  await page.evaluate(({ key, value }) => localStorage.setItem(key, value), { key: `orch.${other.userId}.resources.open`, value: JSON.stringify(['hosts', 'templates', 'agents', 'environments']) });
  const measured = traffic(page);
  const names = () => measured.requests.filter(({ method, url }) => method === 'GET' && url.pathname === '/api/v1/everything'
    && url.searchParams.get('parent') === 'resources' && url.searchParams.get('resource_details') !== 'true');
  try {
    for (const path of [`/resources/hosts/${RUNS_ON.buildHost}`, `/resources/templates/${RUNS_ON.defaultTemplate}`, `/resources/agents/${RUNS_ON.codex}`]) {
      const start = measured.requests.length;
      await page.goto(path); await settled(page);
      await expect(page.locator('[data-resource-navigation]')).toBeVisible();
      expect(measured.requests.length - start, measured.lines().join('\n')).toBeLessThanOrEqual(FIRST_LOAD);
      expect(names()).toEqual([]);
    }
    const agents = page.locator('[data-resource-navigation]').getByRole('treeitem', { name: /^Agents/ });
    await agents.focus(); await agents.press('ArrowRight');
    await expect.poll(() => names().length).toBe(1);
    expect(names()[0]?.url.searchParams.get('kinds')).toBe('agent');
    await expect(page.locator('[data-resource-navigation] [role="treeitem"][aria-level="2"]').first()).toBeVisible();
    const beforeReload = names().length;
    await page.reload(); await settled(page);
    expect(names().slice(beforeReload).map(({ url }) => url.searchParams.get('kinds'))).toEqual(['agent']);
  } finally { measured.stop(); }
});

test('a closed right bar makes no team-chat reads; showing and expanding it shares one sync and one draft', async ({ page }) => {
  await scenario(); await signIn(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`/project/${ACME.web}`); await settled(page);
  if (await page.locator('[data-workbench-bar="right"]').count() > 0) await page.getByRole('button', { name: 'Right bar', exact: true }).click();
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveCount(0); await settled(page);
  const measured = traffic(page);
  const chats = () => measured.requests.filter(({ url }) => /^\/api\/v1\/chats\/[^/]+\/(sync|draft)$/.test(url.pathname));
  try {
    await page.reload(); await settled(page);
    expect(chats(), measured.lines().join('\n')).toEqual([]);
    const initial = measured.requests.length;
    await page.clock.install(); await page.clock.fastForward(IDLE_MS); await settled(page);
    expect(measured.requests.slice(initial)).toEqual([]);
    await page.getByRole('button', { name: 'Right bar', exact: true }).click();
    await expect(page.locator('[data-workbench-bar="right"]')).toBeVisible(); await settled(page);
    expect(chats().filter(({ url }) => url.pathname.endsWith('/sync'))).toHaveLength(1);
    expect(chats().filter(({ url }) => url.pathname.endsWith('/draft'))).toHaveLength(1);
    const expand = page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'Expand', exact: true });
    await expand.click(); await settled(page);
    expect(chats().filter(({ url }) => url.pathname.endsWith('/sync'))).toHaveLength(1);
    expect(chats().filter(({ url }) => url.pathname.endsWith('/draft'))).toHaveLength(1);
  } finally { measured.stop(); }
});
