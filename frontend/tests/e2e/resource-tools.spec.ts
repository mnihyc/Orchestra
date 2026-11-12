import { unjudgedProvenance } from '../doubles/resource-grouping';
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
/** MCP and mount routes show real forms, revision refusals and a volume's explicit destructive confirmation. */
import { expect, type Page, test } from '@playwright/test';
import words from '../../src/i18n/en.json' with { type: 'json' };
import type { EnvironmentMcp, McpServer, Mount, NameHit } from '../../src/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { ADA } from '../doubles/thread-scenes';
import { bodySent, DOUBLE, scenario, signIn } from './support';

const MCP = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a60';
const MOUNT = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a61';
const home = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };

/** Direct, inventory and live reads must all know the same custom fixture identity. */
async function installToolRows(page: Page, rows: { mcp?: McpServer; mount?: Mount }): Promise<void> {
  expect((await page.request.post(`${DOUBLE}/__double/request-feeds/fixtures`, { data: rows })).status()).toBe(204);
}

test.beforeEach(async () => { await scenario(); });

test('Add MCP server opens its real ordinary form without a missing-label crash or visible JSON', async ({ page }) => {
  await signIn(page); await page.goto('/resources/mcp');
  await page.getByRole('button', { name: words.resourceDetails.mcp.add, exact: true }).click();
  await page.getByRole('menuitem', { name: words.mcpImport.create, exact: true }).click();
  const dialog = page.getByRole('dialog', { name: words.resourceDetails.mcp.add, exact: true });
  await expect(dialog.getByRole('textbox', { name: words.resources.edit.name, exact: true })).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: words.resourceDetails.mcp.form.url, exact: true })).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: words.resourceDetails.mcp.form.json, exact: true })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: words.common.add, exact: true })).toHaveAttribute('aria-disabled', 'true');
  await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click(); await expect(dialog).toHaveCount(0);
});

/** The browser consumes a served hit; native HTTP proofs check that Names/Everything build this same address. */
async function openServedName(page: Page, named: Pick<NameHit, 'kind' | 'id' | 'name' | 'href'>) {
  const hit: NameHit = { ...named, tag: null, thread_attention: null, color: null, custom_color: null,
    address: null, username: null, avatar: null, path: [], home_path: [], owner: ADA, archived: false, recent: false };
  await page.route('**/api/v1/search/names**', async (route) => {
    const query = new URL(route.request().url()).searchParams;
    const q = query.get('q') ?? '';
    const kinds = (query.get('kinds') ?? '').split(',');
    await route.fulfill({ json: { items: q === named.name && kinds.includes(named.kind) ? [hit] : [], partial: false } });
  });
  await page.goto('/org/acme');
  await page.getByRole('button', { name: 'Find anything', exact: true }).click();
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill(named.name);
  await finder.getByRole('option', { name: new RegExp(`^${named.name}`) }).click();
  await expect(page).toHaveURL(new RegExp(`${named.href ?? ''}$`));
  await expect(page.getByRole('heading', { name: named.name, exact: true })).toBeVisible();
  await expect(finder).toHaveCount(0);
}

test('a preset opens from its list, rejects invalid JSON, and preserves an edit refused at a stale revision', async ({ page }) => {
  let server: McpServer = { ...unjudgedProvenance(), id: MCP, name: 'code-tools', description: 'Code tools', org_id: ACME.org, owner: ADA, home, address: 'acme.ada.code-tools', machine: null, environment: null, imported: null, spec: { transport: 'stdio', command: ['tools'] }, enabled: true, revision: 7, access: { admin_only: [], participating: true, permissions: ['mcp.view', 'mcp.use', 'mcp.manage'], administers: false, through: [{ kind: 'owner' }] } };
  const writes: Record<string, unknown>[] = [];
  let stale = true;
  await installToolRows(page, { mcp: server });
  await page.route('**/api/v1/mcp-servers**', async (route) => {
    const request = route.request();
    if (request.method() !== 'PATCH') { await route.continue(); return; }
    const body = request.postDataJSON() as Record<string, unknown>;
    writes.push(body);
    if (stale) { await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', status: 409, code: 'conflict.stale_revision', title: 'Changed', detail: 'The server changed first.', request_id: 'stale-request' } }); return; }
    server = { ...server, ...(typeof body.name === 'string' ? { name: body.name } : {}), ...(typeof body.description === 'string' ? { description: body.description } : {}), spec: body.spec ?? server.spec, revision: server.revision + 1 };
    await installToolRows(page, { mcp: server });
    await route.fulfill({ json: server });
  });
  await signIn(page);
  await page.goto('/resources/mcp');
  await page.getByRole('link', { name: 'code-tools', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'code-tools', exact: true })).toBeVisible();
  await openServedName(page, { kind: 'mcp_server', id: MCP, name: server.name, href: `/resources/mcp/${MCP}` });
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit code-tools', exact: true });
  await dialog.locator('summary').filter({ hasText: 'Advanced' }).click();
  await dialog.getByRole('textbox', { name: 'Definition (JSON)', exact: true }).fill('[');
  await expect(dialog.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await dialog.getByRole('textbox', { name: 'Definition (JSON)', exact: true }).fill('{"transport":"stdio","command":["new-tools"]}');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).toContainText(/changed/i);
  await expect(dialog.getByRole('textbox', { name: 'Definition (JSON)', exact: true })).toHaveValue(/new-tools/);
  expect(writes[0]?.expected_revision).toBe(7);
  stale = false;
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator('#mcp-definition')).toContainText('new-tools');
});

test('a mount shows its target and uses, and deleting a volume requires its actual name', async ({ page }) => {
  const mount: Mount = { ...unjudgedProvenance(), id: MOUNT, name: 'datasets', address: 'acme.ada.datasets', description: 'Shared data', org_id: ACME.org, owner: ADA, home, target: '/mnt/data', read_only: true, enabled: true, default_fate: 'as_is', kept: [], revision: 3, access: { admin_only: [], participating: true, permissions: ['mount.view', 'mount.use', 'mount.manage'], administers: false, through: [{ kind: 'owner' }] }, locations: [{ machine: { id: RUNS_ON.laptop, name: 'host-1', online: true, platform_os: 'linux', placement: 'managed' }, host: { id: RUNS_ON.buildHost, name: 'host-1', address: 'host-1' }, kind: 'volume', storage: null, source: 'volume-1', data_kind: null, can_remove: true, created_by: ADA, created_at: '2026-10-14T08:00:00Z' }] };
  let removed = '';
  await installToolRows(page, { mount });
  await page.route('**/api/v1/mounts**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === 'DELETE') { removed = url.searchParams.get('confirm') ?? ''; mount.locations = []; await installToolRows(page, { mount }); await route.fulfill({ status: 204 }); return; }
    if (url.pathname.endsWith('/uses')) { await route.fulfill({ json: { scopes: [], environments: [], hidden: 2 } }); return; }
    await route.continue();
  });
  await signIn(page);
  await page.goto(`/resources/mounts/${MOUNT}`);
  await expect(page.getByRole('heading', { name: 'datasets', exact: true })).toBeVisible();
  await openServedName(page, { kind: 'mount', id: MOUNT, name: mount.name, href: `/resources/mounts/${MOUNT}` });
  await expect(page.getByText('/mnt/data', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: words.resourceDetails.mount.uses, exact: true }).click();
  await expect(page.getByText('2 other uses are not visible to you.', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Locations', exact: true }).click();
  await page.getByRole('region', { name: 'Locations', exact: true }).getByRole('button', { name: 'Delete', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete location', exact: true });
  await expect(dialog.getByRole('button', { name: 'Delete', exact: true })).toBeDisabled();
  await dialog.getByRole('textbox', { name: 'Mount name to confirm', exact: true }).fill('datasets');
  await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect.poll(() => removed).toBe('datasets');
  await expect(page.getByText(/No location is configured for this mount yet/)).toBeVisible();
});

test('the agent override form sends an object under the revision read and keeps invalid text local', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}?tab=profile`);
  await page.locator('[data-resource-details] > summary').click();
  const form = page.getByRole('list', { name: 'Descriptor overrides', exact: true });
  await form.getByRole('button', { name: 'Edit Descriptor overrides', exact: true }).click();
  await form.getByRole('textbox', { name: 'Overridden descriptor members', exact: true }).fill('null');
  await expect(form.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await form.getByRole('textbox', { name: 'Overridden descriptor members', exact: true }).fill('{"capabilities":{"steer":false}}');
  const saved = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname.endsWith(`/agents/${RUNS_ON.codexOnLaptop}`));
  await form.getByRole('button', { name: 'Save', exact: true }).click();
  const body = (await bodySent(await saved)) as { descriptor_overrides: unknown; expected_revision: number };
  expect(body.descriptor_overrides).toEqual({ capabilities: { steer: false } });
  expect(body.expected_revision).toBeGreaterThan(0);
});

test('moving an environment’s home names the new place and changes it under the revision read', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Workspace', exact: true });
  await dialog.getByRole('button', { name: /^Take away / }).click();
  await dialog.getByRole('combobox', { name: 'Belongs to', exact: true }).fill('Fix login');
  await page.getByRole('option', { name: /Fix login/ }).click();
  await expect(dialog).toContainText(/changes the defaults, settings and secret names/);
  const saved = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname.endsWith(`/environments/${RUNS_ON.workspace}`));
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  const body = (await bodySent(await saved)) as { home_node_id: string; expected_revision: number };
  expect(body.home_node_id).toBe(ACME.fixLogin);
  expect(body.expected_revision).toBeGreaterThan(0);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`reviews an HTTP MCP discovery in its own language without importing (${locale}/${theme}/${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const copy = locale === 'en' ? words : chinese;
    const discovery: EnvironmentMcp = { applies: 'next_turn', servers: [], import: { allowed: true, reason: null }, found: [
      { source: 'codex', scope: 'user', name: 'http-proof', transport: 'http', directory: null, imported: null },
    ] };
    await page.route('**/api/v1/environments/*/mcp', (route) => route.fulfill({ json: discovery }));
    const imports: string[] = [];
    page.on('request', (request) => { if (request.method() === 'POST' && /\/environments\/[^/]+\/mcp\/imports$/.test(new URL(request.url()).pathname)) imports.push(request.url()); });
    let rows: unknown;
    await page.route(/\/api\/v1\/environments(?:\?.*)?$/, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      // Capture the actual native list before Chromium can release its response body.
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      rows = await response.json();
      await route.fulfill({ response, json: rows });
    });
    await page.goto('/resources/mcp');
    await page.getByRole('button', { name: copy.resourceDetails.mcp.add, exact: true }).click();
    const loaded = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname.endsWith('/environments'));
    await page.getByRole('menuitem', { name: copy.mcpImport.fromMachine, exact: true }).click();
    const response = await loaded;
    expect(response.ok()).toBe(true);
    if (!Array.isArray(rows)) throw new Error('The native environment list supplies readable rows.');
    const environment: unknown = rows.find((row: unknown) => typeof row === 'object' && row !== null && 'id' in row && row.id === RUNS_ON.workspace);
    if (typeof environment !== 'object' || environment === null || !('name' in environment) || typeof environment.name !== 'string'
      || !('machine' in environment) || typeof environment.machine !== 'object' || environment.machine === null
      || !('name' in environment.machine) || typeof environment.machine.name !== 'string') throw new Error('The import fixture needs its actual environment and machine names.');
    const chooser = page.getByRole('dialog', { name: copy.mcpImport.fromMachine, exact: true });
    await chooser.getByRole('combobox', { name: copy.mcpImport.environment, exact: true }).click();
    await page.getByRole('option', { name: `${environment.machine.name} / ${environment.name}`, exact: true }).click();
    const found = chooser.getByRole('region', { name: copy.mcpImport.found, exact: true });
    await found.getByRole('button', { name: copy.mcpImport.import, exact: true }).click();
    const preview = page.getByRole('dialog', { name: copy.mcpImport.title, exact: true });
    await expect(preview.getByText(copy.mcpImport.transports.http, { exact: true })).toBeVisible();
    await expect(preview.getByText(copy.mcpImport.stayOnMachine, { exact: true })).toBeVisible();
    expect(imports).toEqual([]);
  });
}
