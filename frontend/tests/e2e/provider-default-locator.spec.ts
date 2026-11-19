/** Production provider defaults over native-shaped fixture APIs; successful authority replies are never replaced. */
import { expect, type APIRequestContext, type Locator, type Page, type Request, type TestInfo, test } from '@playwright/test';
import type { AuthResult, CreateProvider, EverythingRowPage, Provider, ProviderWriteReply, ResolvedSetting } from '../../src/generated/api-types.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, PEOPLE, deny, scenario, signIn } from './support.ts';

const double = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const api = `${double}/api/v1`, KEY = 'agent.default_provider_id';
const settingPath = `/api/v1/nodes/${ACME.web}/settings/${KEY}`;
const currentPath = `/api/v1/providers/${RUNS_ON.openAiKey}`;
const PAGE_SIZE = 50; // Existing native Everything page; one extra matching provider requires its cursor.
const OBSERVE_MS = 500; // Two existing debounce intervals expose redundant focus reads, not a latency limit.
interface Call { method: string; path: string; query: Record<string, string> }
interface Write { value: unknown; expected_revision?: number; locked?: boolean }
type SeenWindow = typeof window & { __defaultProviderWrites: Write[] };
const nativeArgs = { scope: 'mine', parent: 'resources', kinds: 'provider', resource_details: 'true', resource_view: 'usable', sort: 'name', direction: 'asc', limit: String(PAGE_SIZE) };

async function fixture(request: APIRequestContext, many = false) {
  await scenario();
  const login = await request.post(`${api}/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
  expect(login.status()).toBe(200); const auth = await login.json() as AuthResult;
  const headers = { Authorization: `Bearer ${auth.access_token}` };
  const existing = await request.get(`${double}${currentPath}`, { headers }); expect(existing.status()).toBe(200);
  const current = await existing.json() as Provider;
  const settings = await request.get(`${api}/nodes/${ACME.web}/settings`, { headers }); expect(settings.status()).toBe(200);
  const before = (await settings.json() as ResolvedSetting[]).find((row) => row.key === KEY);
  if (before?.own === null || before?.own === undefined || before.value !== current.id) throw new Error('The native fixture must retain its own named project default');
  let late = current;
  if (many) for (let n = 1; n <= PAGE_SIZE + 1; n += 1) {
    const body: CreateProvider = { kind: 'api', home_id: ACME.org, name: `Default locator ${String(n).padStart(2, '0')}`, access: 'only_me',
      upstreams: [{ protocol: 'openai-responses', base_url: 'https://example.invalid/v1', options: { auth: 'none' } }] };
    const made = await request.post(`${api}/providers`, { headers: { ...headers, 'Idempotency-Key': `default-locator-${n}` }, data: body });
    expect(made.status()).toBe(200); late = await made.json() as ProviderWriteReply;
  }
  return { current, late, before, headers };
}
function observe(page: Page) {
  const calls: Call[] = [];
  const listener = (request: Request) => { const url = new URL(request.url()); if (url.pathname.startsWith('/api/v1/')) calls.push({ method: request.method(), path: url.pathname, query: Object.fromEntries(url.searchParams) }); };
  page.on('request', listener);
  return { calls, stop: () => page.off('request', listener), choices: () => calls.filter((call) => call.path === '/api/v1/everything' && call.query.resource_view === 'usable') };
}
async function watchWrites(page: Page) {
  await page.addInitScript((path) => {
    const watched = window as SeenWindow; watched.__defaultProviderWrites = [];
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      if (input instanceof Request && input.method === 'PUT' && new URL(input.url).pathname === path) {
        void input.clone().json().then((body: Write) => watched.__defaultProviderWrites.push(body));
      }
      return send(input, init);
    };
  }, settingPath);
}
async function writes(page: Page) { return await page.evaluate(() => (window as SeenWindow).__defaultProviderWrites); }
function answer(page: Page, path: string, method = 'GET', match: (query: URLSearchParams) => boolean = () => true) {
  return page.waitForResponse((response) => { const url = new URL(response.url()); return response.request().method() === method && url.pathname === path && match(url.searchParams); });
}
async function prepare(page: Page, request: APIRequestContext, locale: 'en' | 'zh-CN', width: number, theme: 'light' | 'dark') {
  await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
  expect((await request.post(`${double}/__double/first-steps/appearance`, { data: { locale, theme: 'system' } })).ok()).toBe(true);
  await page.goto(`/project/${ACME.web}/settings?section=policies&setting=${KEY}`);
  await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  const words = locale === 'en' ? english : chinese;
  const row = page.locator(`[data-setting="${KEY}"]`), field = row.getByRole('combobox', { name: words.day100.builtin.agent.default_provider_id.title, exact: true });
  await expect(field).toBeVisible(); return { row, field, words };
}
async function capture(page: Page, info: TestInfo, name: string, input: Locator) {
  await input.scrollIntoViewIfNeeded(); await page.mouse.move(0, 0);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => { const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end); }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
  const geometry = await input.evaluate((element) => {
    const rect = (node: Element) => { const box = node.getBoundingClientRect(); return { left: box.left, right: box.right, top: box.top, bottom: box.bottom }; };
    const row = element.closest('[data-setting-row]');
    const name = row?.querySelector(':scope > div:first-child > div:first-child > span');
    const linked = document.getElementById(element.getAttribute('aria-controls') ?? '');
    const popup = linked?.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
    const list = popup?.querySelector('[role="listbox"]'), footer = popup?.querySelector('[data-combobox-footer]');
    const active = list?.querySelector('[aria-selected="true"]');
    return { field: rect(element), owner: rect(row ?? document.documentElement), name: name === undefined || name === null ? null : rect(name),
      popup: popup === undefined || popup === null ? null : { ...rect(popup), width: popup.clientWidth, scrollWidth: popup.scrollWidth },
      list: list === undefined || list === null ? null : rect(list), footer: footer === undefined || footer === null ? null : rect(footer),
      active: active === undefined || active === null ? null : rect(active), width: innerWidth, height: innerHeight };
  });
  expect(geometry.field.left).toBeGreaterThanOrEqual(Math.max(0, geometry.owner.left) - 1);
  expect(geometry.field.right).toBeLessThanOrEqual(Math.min(geometry.width, geometry.owner.right) + 1); expect(geometry.name).not.toBeNull();
  if (geometry.name !== null) expect(geometry.name.bottom <= geometry.field.top + 1 || geometry.name.right <= geometry.field.left + 1).toBe(true);
  if (geometry.popup !== null) {
    expect(geometry.popup.left).toBeGreaterThanOrEqual(-1); expect(geometry.popup.right).toBeLessThanOrEqual(geometry.width + 1);
    expect(geometry.popup.top).toBeGreaterThanOrEqual(-1); expect(geometry.popup.bottom).toBeLessThanOrEqual(geometry.height + 1);
    expect(geometry.popup.scrollWidth).toBeLessThanOrEqual(geometry.popup.width + 1);
    if (geometry.footer !== null && geometry.list !== null) { expect(geometry.footer.top).toBeGreaterThanOrEqual(geometry.list.bottom - 1); expect(geometry.footer.bottom).toBeLessThanOrEqual(geometry.popup.bottom + 1); }
    if (geometry.active !== null) {
      const viewport = geometry.footer === null ? geometry.popup : geometry.list;
      if (viewport !== null) { expect(geometry.active.top).toBeGreaterThanOrEqual(viewport.top - 1); expect(geometry.active.bottom).toBeLessThanOrEqual(viewport.bottom + 1); }
    }
  }
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const path = info.outputPath(`${name}.png`); await page.screenshot({ path, fullPage: false, caret: 'hide' });
  await info.attach(name, { path, contentType: 'image/png' }); await info.attach(`${name}-geometry`, { body: JSON.stringify(geometry, null, 2), contentType: 'application/json' });
}

test('provider default searches and pages the native usable inventory, then saves and undoes one explicit choice (1440/en/light)', async ({ page, request }, info) => {
  const held = await fixture(request, true); await watchWrites(page); const seen = observe(page);
  try {
    const { row, field, words } = await prepare(page, request, 'en', 1440, 'light');
    await expect(field).toHaveValue(held.current.name); expect(seen.choices()).toHaveLength(0);
    const firstRead = answer(page, '/api/v1/everything', 'GET', (q) => q.get('resource_view') === 'usable');
    await field.click(); const firstReply = await firstRead; expect(firstReply.status()).toBe(200); const first = await firstReply.json() as EverythingRowPage;
    expect(first.items).toHaveLength(PAGE_SIZE); expect(first.next_cursor).not.toBeNull(); expect(first.items.some((item) => item.id === held.late.id)).toBe(false);
    const list = page.getByRole('listbox', { name: words.day100.builtin.agent.default_provider_id.title, exact: true });
    await expect(list.getByRole('option')).toHaveCount(first.items.length + (first.items.some((item) => item.id === held.current.id) ? 0 : 1));
    expect(seen.choices()[0]?.query).toEqual(nativeArgs); await capture(page, info, 'provider-default-browse-1440-en-light', field);
    const searchRead = answer(page, '/api/v1/everything', 'GET', (q) => q.get('q') === 'Default locator' && !q.has('cursor'));
    await field.fill('Default locator'); const searchReply = await searchRead; expect(searchReply.status()).toBe(200); const matching = await searchReply.json() as EverythingRowPage;
    expect(matching.items).toHaveLength(PAGE_SIZE); expect(matching.next_cursor).not.toBeNull(); await expect(list.getByRole('option')).toHaveCount(PAGE_SIZE);
    const more = page.getByRole('dialog', { name: words.day100.builtin.agent.default_provider_id.title, exact: true }).getByRole('button', { name: words.ui.pagination.more, exact: true });
    await field.press('Tab'); await expect(more).toBeFocused();
    const nextRead = answer(page, '/api/v1/everything', 'GET', (q) => q.get('q') === 'Default locator' && q.get('cursor') === matching.next_cursor);
    await more.press('Enter'); const next = await nextRead; expect(next.status()).toBe(200); expect((await next.json() as EverythingRowPage).next_cursor).toBeNull();
    await expect(more).toHaveCount(0); await expect(field).toBeFocused(); await expect(field).toHaveAttribute('aria-expanded', 'true');
    await expect(list.getByRole('option')).toHaveCount(PAGE_SIZE + 1); const active = await field.getAttribute('aria-activedescendant'); await field.press('ArrowDown');
    await expect(field).not.toHaveAttribute('aria-activedescendant', active ?? ''); await page.waitForTimeout(OBSERVE_MS);
    expect(seen.choices()).toHaveLength(3); expect(await writes(page)).toEqual([]); await capture(page, info, 'provider-default-final-page-1440-en-light', field);
    const exactRead = answer(page, '/api/v1/everything', 'GET', (q) => q.get('q') === held.late.name && !q.has('cursor'));
    await field.fill(held.late.name); expect((await exactRead).status()).toBe(200); await expect(list.getByRole('option')).toHaveCount(1);
    await expect(list.getByRole('option', { name: held.late.name, exact: true })).toHaveAccessibleDescription(words.choiceCatalog.dynamic.resource.description);
    expect(await writes(page)).toEqual([]); await capture(page, info, 'provider-default-name-search-1440-en-light', field);
    const savedRead = answer(page, settingPath, 'PUT'); await field.press('Enter'); const savedReply = await savedRead; expect(savedReply.status()).toBe(200);
    const saved = await savedReply.json() as ResolvedSetting; await expect(field).toHaveValue(held.late.name); await expect(field).toHaveAttribute('aria-expanded', 'false');
    await expect.poll(async () => (await writes(page)).length).toBe(1); expect((await writes(page))[0]).toEqual({ value: held.late.id, expected_revision: held.before.own!.revision });
    const undone = answer(page, settingPath, 'PUT'); await row.getByRole('button', { name: words.settings.compact.undoNamed.replace('{{name}}', words.day100.builtin.agent.default_provider_id.title), exact: true }).click();
    expect((await undone).status()).toBe(200); await expect(field).toHaveValue(held.current.name); await expect.poll(async () => (await writes(page)).length).toBe(2);
    expect((await writes(page))[1]).toEqual({ value: held.current.id, locked: held.before.own!.locked, expected_revision: saved.own!.revision });
    await page.waitForTimeout(OBSERVE_MS); expect(seen.choices()).toHaveLength(4);
    expect(seen.calls.filter((call) => call.path === '/api/v1/providers' || call.path === `/api/v1/providers/${held.late.id}`)).toEqual([]);
    await capture(page, info, 'provider-default-undo-1440-en-light', field);
  } finally { seen.stop(); await info.attach('native-provider-default-calls', { body: JSON.stringify({ calls: seen.calls, writes: await writes(page) }, null, 2), contentType: 'application/json' }); }
});

test('provider default retains its current identity through pending/error recovery and cancels a failed search without writing (400/zh/dark)', async ({ page, request }, info) => {
  const held = await fixture(request); await watchWrites(page); const seen = observe(page);
  let release: () => void = () => undefined, entered: () => void = () => undefined, first = true;
  const gate = new Promise<void>((resolve) => { release = resolve; }), arrived = new Promise<void>((resolve) => { entered = resolve; });
  await page.route(`**${currentPath}`, async (route) => {
    if (!first) { await route.fallback(); return; }
    first = false; const native = await route.fetch(); expect(native.status()).toBe(200); entered(); await gate;
    await route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', status: 500,
      title: 'Private provider diagnostic', detail: 'Private provider diagnostic', request_id: 'provider-default-browser' } });
  });
  try {
    const { row, field, words } = await prepare(page, request, 'zh-CN', 400, 'dark'); await arrived;
    await expect(field).toHaveValue(words.places.providerDefault.loading); await capture(page, info, 'provider-default-pending-400-zh-dark', field);
    release(); const area = row.locator('[data-provider-default]'); await expect(area.getByRole('alert')).toBeVisible();
    await expect(field).toHaveValue(words.places.providerDefault.unavailable); await expect(area).not.toContainText('Private provider diagnostic'); await expect(area).not.toContainText(held.current.id);
    await capture(page, info, 'provider-default-current-error-400-zh-dark', field);
    const retried = answer(page, currentPath); await area.getByRole('button', { name: words.common.tryAgain, exact: true }).click(); expect((await retried).status()).toBe(200);
    await expect(field).toHaveValue(held.current.name); await expect(field).toBeFocused(); await expect(field).toHaveAttribute('aria-expanded', 'false');
    await page.waitForTimeout(OBSERVE_MS); expect(seen.calls.filter((call) => call.path === currentPath)).toHaveLength(2); expect(seen.choices()).toHaveLength(0);
    const browse = answer(page, '/api/v1/everything', 'GET', (q) => q.get('resource_view') === 'usable'); await field.click(); expect((await browse).status()).toBe(200);
    await capture(page, info, 'provider-default-recovered-400-zh-dark', field);
    let failSearch = true;
    await page.route('**/api/v1/everything?**', async (route) => {
      const query = new URL(route.request().url()).searchParams;
      if (!failSearch || query.get('q') !== held.current.name || query.get('resource_view') !== 'usable') { await route.fallback(); return; }
      failSearch = false; const native = await route.fetch(); expect(native.status()).toBe(200);
      await route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', status: 500,
        title: 'Private provider search diagnostic', detail: 'Private provider search diagnostic', request_id: 'provider-search-browser' } });
    });
    const failed = answer(page, '/api/v1/everything', 'GET', (q) => q.get('q') === held.current.name); await field.fill(held.current.name); expect((await failed).status()).toBe(500);
    const popup = page.getByRole('dialog', { name: words.day100.builtin.agent.default_provider_id.title, exact: true });
    await expect(popup.getByRole('alert')).toBeVisible(); await expect(popup).not.toContainText('Private provider search diagnostic');
    await capture(page, info, 'provider-default-search-error-400-zh-dark', field);
    const retry = answer(page, '/api/v1/everything', 'GET', (q) => q.get('q') === held.current.name); await popup.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    expect((await retry).status()).toBe(200); await expect(field).toBeFocused(); await expect(page.getByRole('listbox', { name: words.day100.builtin.agent.default_provider_id.title, exact: true }).getByRole('option', { name: held.current.name, exact: true })).toBeVisible();
    await page.waitForTimeout(OBSERVE_MS); expect(seen.choices()).toHaveLength(3); expect(seen.choices().map((call) => call.query)).toEqual([nativeArgs, { ...nativeArgs, q: held.current.name }, { ...nativeArgs, q: held.current.name }]);
    await field.fill(''); await field.press('Escape'); await expect(field).toHaveValue(held.current.name); await field.press('Enter'); await expect(field).toHaveAttribute('aria-expanded', 'false');
    await row.getByRole('button', { name: words.settings.editor.menuOf.replace('{{name}}', words.day100.builtin.agent.default_provider_id.title), exact: true }).click();
    await page.getByRole('menuitem', { name: words.settings.editor.showDetails, exact: true }).click();
    await expect(row.getByLabel(words.settings.editor.detailsOf.replace('{{name}}', words.day100.builtin.agent.default_provider_id.title), { exact: true })).toBeVisible();
    await page.waitForTimeout(OBSERVE_MS); expect(seen.choices()).toHaveLength(3); expect(await writes(page)).toEqual([]);
    await capture(page, info, 'provider-default-details-400-zh-dark', field);
  } finally { release(); seen.stop(); await info.attach('native-provider-default-recovery-calls', { body: JSON.stringify({ calls: seen.calls, writes: await writes(page) }, null, 2), contentType: 'application/json' }); }
});


test('node settings retain the provider default beside guarded usage values and native reader boundaries', async ({ request }) => {
  const held = await fixture(request);
  const read = async (id: string): Promise<ResolvedSetting[]> => {
    const response = await request.get(`${api}/nodes/${id}/settings`, { headers: held.headers });
    expect(response.status()).toBe(200); const rows = await response.json() as ResolvedSetting[];
    expect(new Set(rows.map((row) => row.key)).size).toBe(rows.length); return rows;
  };
  const project = await read(ACME.web), org = await read(ACME.org);
  const usage = (rows: ResolvedSetting[]) => rows.filter((row) => ['usage.rates', 'usage.catalogue', 'usage.catalogue_scale'].includes(row.key));
  expect(usage(project)).toHaveLength(3); expect(project.find((row) => row.key === KEY)).toEqual(held.before);
  const scale = org.find((row) => row.key === 'usage.catalogue_scale');
  if (scale === undefined) throw new Error('The existing usage scale is required');
  const changed = await request.put(`${api}/nodes/${ACME.org}/settings/usage.catalogue_scale`, { headers: held.headers,
    data: { value: '2', expected_revision: scale.own?.revision ?? 0 } });
  expect(changed.status()).toBe(200); const accepted = await changed.json() as ResolvedSetting;
  expect(accepted.value).toBe('2'); expect(accepted.own?.revision).toBe((scale.own?.revision ?? 0) + 1);
  expect((await read(ACME.org)).find((row) => row.key === accepted.key)).toEqual(accepted);
  const inherited = await read(ACME.web);
  expect(inherited.find((row) => row.key === KEY)).toEqual(held.before);
  expect(inherited.find((row) => row.key === accepted.key)).toMatchObject({ value: '2', own: null, source: { scope: 'org', node: null }, writable: false });
  expect(usage(inherited).filter((row) => row.key !== accepted.key)).toEqual(usage(project).filter((row) => row.key !== accepted.key));
  const stale = await request.put(`${api}/nodes/${ACME.org}/settings/usage.catalogue_scale`, { headers: held.headers,
    data: { value: '3', expected_revision: scale.own?.revision ?? 0 } });
  expect(stale.status()).toBe(409); expect(await stale.json()).toMatchObject({ code: 'conflict.stale_revision' });
  expect((await read(ACME.web)).find((row) => row.key === accepted.key)?.value).toBe('2');
  await deny('ada', [`usage.summary@${ACME.web}`, `usage.view@${ACME.web}`]);
  expect((await read(ACME.web)).find((row) => row.key === KEY)).toEqual(held.before);
  expect((await request.get(`${api}/usage/rates?node_id=${ACME.web}`, { headers: held.headers })).status()).toBe(403);
  await deny('ada', [`project.view@${ACME.web}`]);
  expect((await request.get(`${api}/nodes/${ACME.web}/settings`, { headers: held.headers })).status()).toBe(403);
});
