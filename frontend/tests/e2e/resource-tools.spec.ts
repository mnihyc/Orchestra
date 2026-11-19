/** MCP and mount routes show real forms, revision refusals and a volume's explicit destructive confirmation. */
import { expect, type Page, test } from '@playwright/test';
import words from '../../src/i18n/en.json' with { type: 'json' };
import type { McpServer, Mount, NameHit } from '../../src/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { ADA } from '../doubles/thread-scenes';
import { bodySent, scenario, signIn } from './support';

const MCP = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a60';
const MOUNT = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a61';
const home = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };

test.beforeEach(async () => { await scenario(); });

test('Add MCP server opens its real ordinary form without a missing-label crash or visible JSON', async ({ page }) => {
  await signIn(page); await page.goto('/resources/mcp');
  await page.getByRole('button', { name: words.resourceDetails.mcp.add, exact: true }).click();
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
    address: null, username: null, avatar: null, path: [], owner: ADA, archived: false, recent: false };
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
  let server: McpServer = { id: MCP, name: 'code-tools', description: 'Code tools', org_id: ACME.org, owner: ADA, home, address: 'acme.ada.code-tools', machine: null, environment: null, imported: null, spec: { transport: 'stdio', command: ['tools'] }, enabled: true, revision: 7, access: { admin_only: [], participating: true, permissions: ['mcp.view', 'mcp.use', 'mcp.manage'], administers: false, through: [{ kind: 'owner' }] } };
  const writes: Record<string, unknown>[] = [];
  let stale = true;
  await page.route('**/api/v1/mcp-servers**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() === 'PATCH') {
      const body = request.postDataJSON() as Record<string, unknown>;
      writes.push(body);
      if (stale) { await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', status: 409, code: 'conflict.stale_revision', title: 'Changed', detail: 'The server changed first.', request_id: 'stale-request' } }); return; }
      server = { ...server, name: String(body.name), description: String(body.description), spec: body.spec ?? server.spec, revision: 8 };
    }
    await route.fulfill({ json: path.endsWith('/mcp-servers') ? [{ ...server, content_hidden: false }] : server });
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
  const mount: Mount = { id: MOUNT, name: 'datasets', address: 'acme.ada.datasets', description: 'Shared data', org_id: ACME.org, owner: ADA, home, target: '/mnt/data', read_only: true, enabled: true, default_fate: 'as_is', kept: [], revision: 3, access: { admin_only: [], participating: true, permissions: ['mount.view', 'mount.use', 'mount.manage'], administers: false, through: [{ kind: 'owner' }] }, locations: [{ machine: { id: RUNS_ON.laptop, name: 'host-1', online: true, platform_os: 'linux', placement: 'managed' }, host: { id: RUNS_ON.buildHost, name: 'host-1', address: 'host-1' }, kind: 'volume', storage: null, source: 'volume-1', data_kind: null, can_remove: true, created_by: ADA, created_at: '2026-10-14T08:00:00Z' }] };
  let removed = '';
  await page.route('**/api/v1/mounts**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === 'DELETE') { removed = url.searchParams.get('confirm') ?? ''; mount.locations = []; await route.fulfill({ status: 204 }); return; }
    if (url.pathname.endsWith('/uses')) { await route.fulfill({ json: { scopes: [], environments: [], hidden: 2 } }); return; }
    await route.fulfill({ json: url.pathname.endsWith('/mounts') ? [{ ...mount, content_hidden: false }] : mount });
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
  const dialog = page.getByRole('dialog', { name: 'Remove location', exact: true });
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
