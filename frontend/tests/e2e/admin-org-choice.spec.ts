/** Production S019 workflow, native fixture APIs and real admin-mode UI; successful replies are never replaced. */
import { expect, type APIRequestContext, type Locator, type Page, type Request, type TestInfo, test } from '@playwright/test';
import type { AuthResult, IdentityProvider, Org, OrgPage } from '../../src/generated/api-types.ts';
import words from '../../src/i18n/en.json' with { type: 'json' };
import { asAdmin } from './admin-mode-support.ts';
import { PEOPLE, scenario, signIn } from './support.ts';

const api = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/api/v1`;
const PAGE_SIZE = 50; // Existing api/admin.ts ADMIN_PAGE_SIZE; one extra real org tests continuation.
const OBSERVE_MS = 500; // Two existing debounce periods, not a latency requirement.
interface Call { method: string; path: string; query: Record<string, string> }
type SeenWindow = typeof window & { __idpWrites: unknown[] };

async function fixture(request: APIRequestContext, many = false) {
  await scenario();
  const login = await request.post(`${api}/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
  expect(login.status()).toBe(200); const auth = await login.json() as AuthResult;
  const headers = { Authorization: `Bearer ${auth.access_token}` };
  expect((await request.put(`${api}/me/admin-mode`, { headers, data: { as_admin: true } })).status()).toBe(200);
  const make = async (name: string, slug: string): Promise<Org> => {
    const response = await request.post(`${api}/orgs`, { headers: { ...headers, 'Idempotency-Key': `idp-${slug}` }, data: { name, slug } });
    expect(response.status()).toBe(200); return await response.json() as Org;
  };
  const home = await make('Locator home', 'locator-home');
  const departmentReply = await request.post(`${api}/orgs/${home.id}/orgs`, {
    headers: { ...headers, 'Idempotency-Key': 'idp-stored-department' }, data: { name: 'Stored department', slug: 'stored' },
  });
  expect(departmentReply.status()).toBe(200); const department = await departmentReply.json() as Org;
  let late = home;
  if (many) for (let n = 1; n <= PAGE_SIZE + 1; n += 1) late = await make(`Locator ${String(n).padStart(2, '0')}`, `locator-${n}`);
  const providers = await request.get(`${api}/admin/identity-providers`, { headers }); expect(providers.status()).toBe(200);
  const provider = (await providers.json() as IdentityProvider[])[0];
  if (provider === undefined) throw new Error('The normal native fixture has no IDP to edit');
  const current = many ? home : department;
  const chosen = await request.patch(`${api}/admin/identity-providers/${provider.id}`, { headers,
    data: { expected_revision: provider.revision, default_org_id: current.id, default_role: 'developer' } });
  expect(chosen.status()).toBe(200);
  return { provider: await chosen.json() as IdentityProvider, department, late, home, current, headers };
}

function observe(page: Page) {
  const calls: Call[] = [];
  const listener = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/')) calls.push({ method: request.method(), path: url.pathname, query: Object.fromEntries(url.searchParams) });
  };
  page.on('request', listener);
  return { calls, stop: () => page.off('request', listener), orgs: () => calls.filter((call) => call.path === '/api/v1/admin/orgs') };
}
async function watchBodies(page: Page) {
  await page.addInitScript(() => {
    const seen = window as SeenWindow; seen.__idpWrites = [];
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      if (input instanceof Request && input.method === 'PATCH' && /^\/api\/v1\/admin\/identity-providers\/[^/]+$/u.test(new URL(input.url).pathname)) {
        void input.clone().json().then((body: unknown) => seen.__idpWrites.push(body), () => seen.__idpWrites.push('Unreadable JSON'));
      }
      return send(input, init);
    };
  });
}
async function writes(page: Page) { return await page.evaluate(() => (window as SeenWindow).__idpWrites); }
async function prepare(page: Page, width: number, theme: 'light' | 'dark') {
  await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  await signIn(page); await asAdmin(page, true); await page.goto('/admin/sign-in');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en'); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
}
async function edit(page: Page, provider: IdentityProvider) {
  await page.getByRole('button', { name: words.admin.idp.menuOf.replace('{{name}}', provider.name), exact: true }).click();
  await page.getByRole('menuitem', { name: words.admin.idp.edit, exact: true }).click();
  const dialog = page.getByRole('dialog', { name: words.admin.idp.editTitle.replace('{{name}}', provider.name), exact: true });
  return { dialog, field: dialog.getByRole('combobox', { name: words.admin.idp.defaultOrg, exact: true }), area: dialog.locator('[data-admin-org-choice]') };
}
function answer(page: Page, path: string, method = 'GET', match: (query: URLSearchParams) => boolean = () => true) {
  return page.waitForResponse((response) => { const url = new URL(response.url()); return response.request().method() === method && url.pathname === path && match(url.searchParams); });
}
async function capture(page: Page, info: TestInfo, name: string, input: Locator) {
  await input.scrollIntoViewIfNeeded(); await page.mouse.move(0, 0);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => {
      const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end);
    }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
  const geometry = await input.evaluate((element) => {
    const rect = (node: Element) => { const box = node.getBoundingClientRect(); return { left: box.left, right: box.right, top: box.top, bottom: box.bottom }; };
    const label = document.getElementById(element.getAttribute('aria-labelledby') ?? '');
    const list = document.getElementById(element.getAttribute('aria-controls') ?? '');
    const popup = list?.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
    const options = popup?.querySelector('[role="listbox"]');
    const footer = popup?.querySelector('[data-combobox-footer]');
    const active = options?.querySelector('[role="option"][aria-selected="true"]');
    return { input: rect(element), label: label === null ? null : rect(label), owner: rect(element.closest('[role="dialog"]') ?? document.documentElement),
      popup: popup === null || popup === undefined ? null : { ...rect(popup), width: popup.clientWidth, scrollWidth: popup.scrollWidth },
      options: options === null || options === undefined ? null : rect(options), footer: footer === null || footer === undefined ? null : rect(footer),
      active: active === null || active === undefined ? null : rect(active), width: innerWidth, height: innerHeight };
  });
  expect(geometry.label).not.toBeNull();
  expect(geometry.input.left).toBeGreaterThanOrEqual(Math.max(0, geometry.owner.left) - 1);
  expect(geometry.input.right).toBeLessThanOrEqual(Math.min(geometry.width, geometry.owner.right) + 1);
  if (geometry.label !== null) expect(geometry.label.bottom).toBeLessThanOrEqual(geometry.input.top + 1);
  const role = await page.getByRole('combobox', { name: words.admin.idp.defaultRole, exact: true }).boundingBox();
  // Compare the two controls only when the existing form puts them beside one another.
  if (role !== null && role.x >= geometry.input.right) expect(Math.abs(role.y - geometry.input.top)).toBeLessThanOrEqual(1);
  if (geometry.popup !== null) {
    expect(geometry.popup.left).toBeGreaterThanOrEqual(-1); expect(geometry.popup.right).toBeLessThanOrEqual(geometry.width + 1);
    expect(geometry.popup.top).toBeGreaterThanOrEqual(-1); expect(geometry.popup.bottom).toBeLessThanOrEqual(geometry.height + 1);
    expect(geometry.popup.scrollWidth).toBeLessThanOrEqual(geometry.popup.width + 1);
    if (geometry.footer !== null && geometry.options !== null) {
      expect(geometry.footer.bottom).toBeLessThanOrEqual(geometry.popup.bottom + 1);
      expect(geometry.footer.top).toBeGreaterThanOrEqual(geometry.options.bottom - 1);
    }
    if (geometry.active !== null && geometry.options !== null) {
      expect(geometry.active.top).toBeGreaterThanOrEqual(geometry.options.top - 1);
      expect(geometry.active.bottom).toBeLessThanOrEqual(geometry.options.bottom + 1);
    }
  }
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const path = info.outputPath(`${name}.png`); await page.screenshot({ path, fullPage: false, caret: 'hide' });
  await info.attach(name, { path, contentType: 'image/png' }); await info.attach(`${name}-geometry`, { body: JSON.stringify(geometry, null, 2), contentType: 'application/json' });
}

test('admin org locator pages the real inventory, selects a department and writes only after Save (1440/light)', async ({ page, request }, info) => {
  const held = await fixture(request, true); await watchBodies(page); await prepare(page, 1440, 'light'); const seen = observe(page);
  try {
    const { dialog, field } = await edit(page, held.provider);
    await expect(field).toHaveValue(held.current.name); expect(seen.orgs()).toHaveLength(0);
    const firstRead = answer(page, '/api/v1/admin/orgs'); await field.click(); const firstReply = await firstRead;
    expect(firstReply.status()).toBe(200); const first = await firstReply.json() as OrgPage;
    expect(first.items).toHaveLength(PAGE_SIZE); expect(first.next_cursor).not.toBeNull(); expect(first.items.some((org) => org.id === held.late.id)).toBe(false);
    const options = page.getByRole('listbox', { name: words.admin.idp.defaultOrg, exact: true });
    expect(first.items.some((org) => org.id === held.current.id)).toBe(false);
    await expect(options.getByRole('option')).toHaveCount(first.items.length + 2); // None + the independently read current organization.
    await expect(options.getByRole('option', { name: words.admin.idp.noOrg, exact: true })).toHaveAccessibleDescription('');
    await capture(page, info, 'org-native-first-page-1440-light', field);
    const popup = page.getByRole('dialog', { name: words.admin.idp.defaultOrg, exact: true });
    const more = popup.getByRole('button', { name: words.ui.pagination.more, exact: true });
    const nextRead = answer(page, '/api/v1/admin/orgs', 'GET', (q) => q.get('cursor') === first.next_cursor); await more.click();
    const nextReply = await nextRead; expect(nextReply.status()).toBe(200); expect((await nextReply.json() as OrgPage).next_cursor).toBeNull();
    await expect(more).toHaveCount(0); await expect(field).toBeFocused(); await expect(field).toHaveAttribute('aria-expanded', 'true');
    const active = await field.getAttribute('aria-activedescendant'); await field.press('ArrowDown');
    await expect(field).not.toHaveAttribute('aria-activedescendant', active ?? ''); expect(await writes(page)).toEqual([]);
    await expect(options.getByRole('option', { name: held.late.name, exact: true })).toBeVisible();
    await page.waitForTimeout(OBSERVE_MS); expect(seen.orgs()).toHaveLength(2);
    await capture(page, info, 'org-native-next-page-1440-light', field);
    const matchingRead = answer(page, '/api/v1/admin/orgs', 'GET', (q) => q.get('q') === 'Locator' && !q.has('cursor'));
    await field.fill('Locator'); const matching = await matchingRead; expect(matching.status()).toBe(200);
    const matchingPage = await matching.json() as OrgPage; expect(matchingPage.next_cursor).not.toBeNull();
    await expect(options.getByRole('option')).toHaveCount(PAGE_SIZE);
    await field.press('Tab'); await expect(popup.getByRole('button', { name: words.resources.bulk.clear, exact: true })).toBeFocused();
    await page.keyboard.press('Tab'); await expect(more).toBeFocused();
    const keyboardPage = answer(page, '/api/v1/admin/orgs', 'GET', (q) => q.get('q') === 'Locator' && q.get('cursor') === matchingPage.next_cursor);
    await more.press('Enter'); expect((await keyboardPage).status()).toBe(200); await expect(field).toBeFocused();
    await page.waitForTimeout(OBSERVE_MS); expect(seen.orgs()).toHaveLength(4);
    await capture(page, info, 'org-native-keyboard-page-1440-light', field);
    const searchRead = answer(page, '/api/v1/admin/orgs', 'GET', (q) => q.get('q') === held.late.name && !q.has('cursor'));
    await field.fill(held.late.name); expect((await searchRead).status()).toBe(200);
    await expect(options.getByRole('option')).toHaveCount(1); await capture(page, info, 'org-native-name-search-1440-light', field);
    expect(await writes(page)).toEqual([]); await field.press('Enter'); await expect(field).toHaveValue(held.late.name);
    const departmentRead = answer(page, '/api/v1/admin/orgs', 'GET', (q) => q.get('q') === held.department.name && !q.has('cursor'));
    await field.click(); await field.fill(held.department.name); const departmentReply = await departmentRead; expect(departmentReply.status()).toBe(200);
    expect((await departmentReply.json() as OrgPage).items.map((org) => org.id)).toEqual([held.department.id]);
    await expect(options.getByRole('option', { name: held.department.name, exact: true })).toBeVisible();
    await capture(page, info, 'org-native-department-search-1440-light', field);
    await field.press('Enter'); await expect(field).toHaveValue(held.department.name);
    expect(await writes(page)).toEqual([]);
    const savePath = `/api/v1/admin/identity-providers/${held.provider.id}`, saved = answer(page, savePath, 'PATCH');
    await dialog.getByRole('button', { name: words.admin.idp.save, exact: true }).click(); expect((await saved).status()).toBe(200);
    await expect.poll(async () => (await writes(page)).length).toBe(1);
    expect((await writes(page))[0]).toMatchObject({ expected_revision: held.provider.revision, default_org_id: held.department.id, default_role: 'developer', client_id: held.provider.client_id });
    expect((await writes(page))[0]).not.toHaveProperty('client_secret'); await expect(dialog).toHaveCount(0);
  } finally { seen.stop(); await info.attach('native-org-page-calls', { body: JSON.stringify({ calls: seen.calls, writes: await writes(page) }, null, 2), contentType: 'application/json' }); }
});

test('admin org current lookup survives pending/failure, retry and clear without an implicit save (400/dark)', async ({ page, request }, info) => {
  const held = await fixture(request); await watchBodies(page); await prepare(page, 400, 'dark'); const seen = observe(page);
  let release: () => void = () => undefined, entered: () => void = () => undefined, first = true;
  const gate = new Promise<void>((resolve) => { release = resolve; }), arrived = new Promise<void>((resolve) => { entered = resolve; });
  const nodePath = `/api/v1/nodes/${held.department.id}`;
  await page.route(`**${nodePath}`, async (route) => {
    if (!first) { await route.fallback(); return; }
    first = false; const response = await route.fetch(); expect(response.status()).toBe(200); entered(); await gate;
    await route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', status: 500,
      title: 'Private organization diagnostic', detail: 'Private organization diagnostic', request_id: 'idp-browser' } });
  });
  try {
    const { dialog, field, area } = await edit(page, held.provider); await arrived;
    await expect(field).toHaveValue(words.common.loading); await expect(area.getByRole('button', { name: words.resources.bulk.clear, exact: true })).toBeEnabled();
    await capture(page, info, 'org-current-pending-400-dark', field); release(); await expect(area.getByRole('alert')).toBeVisible();
    await expect(field).toHaveValue(words.settings.resourceChoice.unavailable); await expect(area).not.toContainText('Private organization diagnostic');
    await capture(page, info, 'org-current-failed-400-dark', field);
    const retry = answer(page, nodePath); await area.getByRole('button', { name: words.common.tryAgain, exact: true }).click(); expect((await retry).status()).toBe(200);
    await expect(field).toHaveValue(held.department.name); await page.waitForTimeout(OBSERVE_MS);
    expect(seen.calls.filter((call) => call.path === nodePath)).toHaveLength(2); expect(seen.orgs()).toHaveLength(0);
    const browse = answer(page, '/api/v1/admin/orgs'); await field.click(); expect((await browse).status()).toBe(200);
    await expect(page.getByRole('option', { name: held.department.name, exact: true })).not.toHaveAttribute('aria-disabled', 'true');
    await capture(page, info, 'org-current-recovered-400-dark', field);
    await page.getByRole('dialog', { name: words.admin.idp.defaultOrg, exact: true }).getByRole('button', { name: words.resources.bulk.clear, exact: true }).click();
    await expect(field).toHaveValue(words.admin.idp.noOrg); await expect(dialog.getByRole('combobox', { name: words.admin.idp.defaultRole, exact: true })).toBeDisabled();
    await expect(field).toHaveAttribute('aria-expanded', 'false'); await expect(field).toBeFocused();
    await page.waitForTimeout(OBSERVE_MS); expect(seen.orgs()).toHaveLength(1);
    expect(await writes(page)).toEqual([]); await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
  } finally { release(); seen.stop(); await info.attach('native-org-recovery-calls', { body: JSON.stringify(seen.calls, null, 2), contentType: 'application/json' }); }
});

test('active admin-org fixture preserves native department, name, keyset ordering and administrator boundaries', async ({ request }) => {
  const held = await fixture(request);
  const make = async (name: string, slug: string, parent?: string): Promise<Org> => {
    const response = await request.post(`${api}/orgs${parent === undefined ? '' : `/${parent}/orgs`}`, {
      headers: { ...held.headers, 'Idempotency-Key': `native-${slug}` }, data: { name, slug },
    });
    expect(response.status()).toBe(200); return await response.json() as Org;
  };
  const a = await make('Native ordering', 'slug-only-native-probe');
  const b = await make('native ordering', 'same-name-department', held.home.id);
  const bmp = await make('Native ordering \uE000', 'bmp-order');
  const supplementary = await make('Native ordering 😀', 'supplementary-order');
  const read = async (query: Record<string, string>): Promise<OrgPage> => {
    const response = await request.get(`${api}/admin/orgs?${new URLSearchParams(query).toString()}`, { headers: held.headers });
    expect(response.status()).toBe(200); return await response.json() as OrgPage;
  };
  const first = await read({ q: 'NATIVE ORDERING', org_kind: 'team', limit: '2' });
  const tied = [a, b].sort((one, other) => one.id < other.id ? -1 : one.id > other.id ? 1 : 0);
  expect(first.items.map((org) => org.id)).toEqual(tied.map((org) => org.id));
  expect(first.items.find((org) => org.id === b.id)).toMatchObject({ parent_id: held.home.id, depth: 1 });
  const last = tied.at(-1); if (last === undefined) throw new Error('The test has no cursor anchor');
  expect(first.next_cursor).toBe(`t|${last.id}|${last.name}`);
  // Inserting before the cursor would duplicate an earlier row with an offset pager.
  await make('! Native ordering', 'before-cursor');
  const next = await read({ q: 'NATIVE ORDERING', org_kind: 'team', limit: '2', cursor: first.next_cursor ?? '' });
  expect(next.items.map((org) => org.id)).toEqual([bmp.id, supplementary.id]); expect(next.next_cursor).toBeNull();
  expect((await read({ q: 'slug-only-native-probe' })).items).toEqual([]); // Native q matches names, not slugs.
  expect((await read({ q: 'Native ordering', org_kind: 'personal' })).items).toEqual([]);
  expect((await request.get(`${api}/admin/orgs?cursor=broken`, { headers: held.headers })).status()).toBe(400);
  const cleared = await request.patch(`${api}/admin/identity-providers/${held.provider.id}`, { headers: held.headers,
    data: { expected_revision: held.provider.revision, default_org_id: null, default_role: null } });
  expect(cleared.status()).toBe(200); expect(await cleared.json()).toMatchObject({ default_org_id: null, default_role: null, revision: held.provider.revision + 1 });
  expect((await request.put(`${api}/me/admin-mode`, { headers: held.headers, data: { as_admin: false } })).status()).toBe(200);
  expect((await request.get(`${api}/admin/orgs?q=Native`, { headers: held.headers })).status()).toBe(403);
});
