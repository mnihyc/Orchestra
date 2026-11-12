/** Kept files, actual stopped restoration jobs, permanent removal races and truthful failure/removal wording. */
import { expect, test, type Locator, type Page } from '@playwright/test';
import { RUNS_ON, TRACE } from '../doubles/fixtures';
import { storageCleanupPlan } from '../doubles/storage-report';
import { ACME, DOUBLE, PEOPLE, replySent, scenario, signIn, arrived } from './support';
import type { CleanupPlan, DeletedEnvironmentPage } from '../../src/generated/api-types';

async function retention(body: Record<string, unknown>) { const response = await fetch(`${DOUBLE}/__double/trace/retention`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }); expect(response.ok).toBe(true); }
async function state() { return (await fetch(`${DOUBLE}/__double/trace/state`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).json() as Promise<{ requests: { method: string; path: string; query: Record<string, string>; body: Record<string, unknown> }[] }>; }
async function deleted(page: Page) { await page.goto(`/project/${ACME.web}`); await page.getByRole('button', { name: /^Actions for Web(?: · .+)?$/, exact: true }).first().click(); await page.getByRole('menuitem', { name: 'Recently deleted…', exact: true }).click(); await expect(page.getByRole('dialog', { name: /^Recently deleted in the project Web, in / })).toBeVisible(); }
const envRow = (page: Page, id: string = TRACE.environment) => page.locator(`[data-kept-environment="${id}"]`);
const stopped = (scope: Page | Locator) => scope.getByRole('status').filter({ hasText: 'Restoration returns Stopped.' });
const deadline = (page: Page) => envRow(page).getByText(/^Kept until /);
async function expectKeptFailure(page: Page, reason: string, until: string) {
  // Jobs use the shared localized refusal; the refreshed retained facts still explain the actual failure.
  await expect(page.getByRole('region', { name: 'Kept after removal', exact: true }).getByRole('status').getByRole('alert')).toHaveText('The environment is not ready at the moment. Try again shortly.');
  await expect(envRow(page).getByText(reason, { exact: true })).toBeVisible();
  await expect(envRow(page).getByText('Kept', { exact: true })).toBeVisible();
  await expect(deadline(page)).toHaveText(until);
  await expect(envRow(page).getByRole('button', { name: 'Bring it back', exact: true })).toBeEnabled();
  await expect(stopped(page)).toHaveCount(0);
}
async function hostKept(page: Page) {
  // Host retained data is now a named section under Storage.
  await page.getByRole('tab', { name: 'Storage', exact: true }).click();
  const sections = page.getByRole('navigation', { name: 'Storage', exact: true });
  await expect(sections).toBeVisible();
  const link = sections.getByRole('link', { name: 'Kept after removal', exact: true });
  if (await link.isVisible()) await link.click();
  else {
    await sections.getByRole('combobox', { name: 'Storage', exact: true }).click();
    await page.getByRole('option', { name: 'Kept after removal', exact: true }).click();
  }
  await expect(page.locator('[data-resource-section="kept"]')).toBeVisible();
}
test.beforeEach(async () => { await scenario(); });

test('Recently deleted includes guarded nodes, documents, measured and unknown environment size, host, storage and phase', async ({ page }) => {
  await signIn(page); await deleted(page); await expect(page.getByText('Archived experiment', { exact: true })).toBeVisible(); await expect(page.locator(`[data-kept-document="${TRACE.document}"]`)).toBeVisible(); await expect(envRow(page)).toContainText('2 GiB'); await expect(envRow(page)).toContainText('host-1'); await expect(envRow(page)).toContainText('Kept');
  const unmeasuredSize = envRow(page, TRACE.unknownEnvironment).getByText('Size', { exact: true }).locator('xpath=following-sibling::dd[1]');
  await expect(unmeasuredSize).toHaveText('Not recorded');
  await envRow(page).getByRole('link', { name: 'Look at it' }).click(); await expect(page).toHaveURL(`/inspect/environment/${TRACE.environment}`); await expect(page.getByText('Read-only inspection. Reading changes no read marks or notifications.')).toBeVisible(); await expect(page.getByRole('button', { name: 'Start', exact: true })).toHaveCount(0); const traffic = await state(); expect(traffic.requests.filter((request) => request.path.endsWith('/start') || request.path.endsWith('/files'))).toEqual([]);
});

test('Bring it back shows its real job and only reports Stopped after the server returns it', async ({ page }) => {
  await signIn(page); await deleted(page); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click();
  await expect(stopped(page)).toContainText('Done');
  await expect(stopped(page).getByRole('link', { name: 'Open environment' })).toHaveAttribute('href', `/resources/environments/${TRACE.environment}`);
  const traffic = await state(); const request = traffic.requests.find((one) => one.path === `/environments/${TRACE.environment}/restore`)!; expect(request.body.expected_revision).toBe(1); expect(traffic.requests.filter((one) => one.path.endsWith('/start') || one.path === `/nodes/${TRACE.thread}/restore`)).toEqual([]);
});

test('an environment can also be restored from its separately authorized audit row', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted&action=environment.deleted'); const row = page.locator(`[data-trace-object="${TRACE.environment}"]`); await row.getByRole('button', { name: 'Bring it back', exact: true }).click(); const dialog = page.getByRole('dialog'); await dialog.getByRole('button', { name: 'Bring back', exact: true }).click();
  await expect(stopped(dialog)).toContainText('Done');
});

test('an environment manager who is not a home admin may restore but has no early purge act', async ({ page }) => {
  await signIn(page, PEOPLE.dora); await deleted(page); await expect(envRow(page).getByRole('button', { name: 'Bring it back' })).toBeVisible(); await expect(page.getByRole('button', { name: 'Delete permanently', exact: true })).toHaveCount(0);
  // A direct permanent-removal request still uses intrinsic home authority.
  const auth = await page.request.post('/api/v1/auth/login', { data: { login: PEOPLE.dora.login, password: PEOPLE.dora.password } }); const grant = await auth.json() as { access_token: string }; const purge = await page.request.delete(`/api/v1/environments/${TRACE.environment}/purge?confirm=Kept+workspace&expected_revision=1`, { headers: { Authorization: `Bearer ${grant.access_token}` } }); expect(purge.status()).toBe(403);
});

test('Remove for good confirms the name and actual size; a purge that wins refuses stale restoration', async ({ page }) => {
  await signIn(page);
  const listed = arrived(page.waitForResponse((response) => response.ok() && new URL(response.url()).pathname === `/api/v1/nodes/${ACME.web}/environments/deleted`));
  await deleted(page);
  const { items } = await (await listed).json() as DeletedEnvironmentPage;
  const retained = items.find((item) => item.environment.id === TRACE.environment);
  const base = storageCleanupPlan(['recovery']);
  const recovery = base.items[0];
  if (retained === undefined || recovery === undefined) throw new Error('The fixture needs the retained environment and its cleanup plan.');
  const name = retained.environment.name;
  const id = `environment:${TRACE.environment}`;
  const resource = { kind: 'environment', id: TRACE.environment } as const;
  const plan: CleanupPlan = { ...base, confirmation_name: name, items: [{
    ...recovery, selected: 'purge', record: { ...recovery.record, id, name, resource, bytes: retained.removal.bytes, facts: null },
  }] };
  // The current cleanup contract uses an expiring plan. Its fake commit uses the existing guarded purge to change retained state.
  await page.route('**/api/v1/storage/cleanup/preflight', async (route) => {
    expect(route.request().postDataJSON()).toEqual({ scope: 'me', resource, items: [id] });
    await route.fulfill({ json: plan });
  });
  await page.route('**/api/v1/storage/cleanup', async (route) => {
    expect(route.request().postDataJSON()).toEqual({ plan_id: plan.id, choices: [{ id, choice: 'purge' }], confirmation_name: name, notify: false });
    const purge = new URL(`/api/v1/environments/${TRACE.environment}/purge`, route.request().url());
    purge.searchParams.set('confirm', name);
    purge.searchParams.set('expected_revision', String(retained.removal.revision));
    await route.fulfill({ response: await route.fetch({ url: purge.href, method: 'DELETE' }) });
  });
  await envRow(page).getByRole('button', { name: 'Delete permanently', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Clean up…', exact: true });
  await expect(dialog.getByRole('table', { name: 'Cleanup plan', exact: true })).toContainText('2 GiB');
  const confirm = dialog.getByRole('button', { name: 'Clean up…', exact: true });
  await expect(confirm).toBeDisabled();
  await dialog.getByRole('textbox', { name: `Type ${name} to erase data`, exact: true }).fill(name);
  await expect(confirm).toBeEnabled();
  await confirm.click();
  const traffic = await state(); const purge = traffic.requests.find((one) => one.path.endsWith('/purge'))!; expect(purge.query.confirm).toBe('Kept workspace'); expect(purge.query.expected_revision).toBe('1');
  await expect(page.getByRole('region', { name: 'Kept after removal', exact: true }).getByRole('status')).toHaveText('Done');
  await expect(dialog).toHaveCount(0);
  await page.goto(`/inspect/environment/${TRACE.environment}`); await expect(page.getByText('Could not look at this deleted thing')).toBeVisible();
});

test('a purge already running and changed authority refuse a stale Bring it back without inventing an empty replacement', async ({ page }) => {
  await signIn(page); await deleted(page); await retention({ environment: TRACE.environment, phase: 'purging', hold_jobs: true });
  const refused = page.waitForResponse((response) => response.status() === 409 && new URL(response.url()).pathname.endsWith(`/${TRACE.environment}/restore`));
  await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click();
  const { code, job } = await replySent(await refused) as { code: string; job: { id: string } };
  expect(code).toBe('resource.in_use');
  await expect.poll(async () => (await state()).requests.some((request) => request.method === 'GET' && request.path === `/jobs/${job.id}`)).toBe(true);
  await expect(page.getByRole('region', { name: 'Kept after removal', exact: true }).getByRole('status')).toBeVisible();
  await expect(envRow(page).getByRole('button', { name: 'Bring it back', exact: true })).toHaveCount(0);
  await expect(envRow(page)).toBeVisible();
  await expect(stopped(page)).toHaveCount(0);
  await retention({ username: 'ada', can_restore: false, can_purge: false }); await page.reload(); await deleted(page); await expect(page.getByRole('button', { name: 'Delete permanently', exact: true })).toHaveCount(0);
});

test('a missing host keeps the deadline and failure job on the host and in Inbox', async ({ page }) => {
  const reason = 'host-1 is not reachable; the kept volumes remain.';
  await signIn(page); await retention({ failure: reason }); await deleted(page);
  const until = await deadline(page).innerText();
  await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click();
  await expectKeptFailure(page, reason, until);
  await page.goto('/inbox'); await page.getByRole('link', { name: /Kept files need attention/ }).click();
  await expect(page).toHaveURL(/\/resources\/hosts\//);
  await hostKept(page);
  // The host reports the localized final state rather than a protocol id and phase string.
  await expect(envRow(page).getByText('Failed', { exact: true })).toBeVisible();
  await expect(deadline(page)).toHaveText(until);
  await expect(page.getByRole('button', { name: 'Delete permanently', exact: true })).toHaveCount(0);
});

test('a stale removal revision and a full disk keep the files deleted with the original deadline', async ({ page }) => {
  await signIn(page); await deleted(page);
  const until = await deadline(page).innerText();
  await retention({ environment: TRACE.environment, stale: true }); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(envRow(page).getByRole('alert')).toBeVisible(); await expect(stopped(page)).toHaveCount(0);
  const reason = 'The host has no free disk space. Kept volumes remain until their original day.';
  await retention({ failure: reason }); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click();
  await expectKeptFailure(page, reason, until);
});

test('a host manager alone reads safe retained facts and has no restore or early purge control', async ({ page }) => {
  await retention({ host_manager: 'vera' }); await signIn(page, PEOPLE.vera); await page.goto('/resources/hosts/00000000-0000-4000-8025-000000000001'); await hostKept(page); await expect(envRow(page)).toContainText('Private environment'); await expect(page.getByText('Kept workspace', { exact: true })).toHaveCount(0); await expect(page.getByRole('button', { name: 'Delete permanently', exact: true })).toHaveCount(0); await expect(page.getByRole('button', { name: 'Bring it back', exact: true })).toHaveCount(0);
});

test('QS-013 the host detail uses its served kept_data and makes no speculative retained request', async ({ page }) => {
  await signIn(page);
  const unsupported: string[] = [];
  page.on('request', (request) => { if (/\/hosts\/[^/]+\/retained$/.test(new URL(request.url()).pathname)) unsupported.push(request.url()); });
  const reply = arrived(page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/hosts/00000000-0000-4000-8025-000000000001'));
  await page.goto('/resources/hosts/00000000-0000-4000-8025-000000000001');
  const wire: unknown = await (await reply).json();
  expect(wire).toMatchObject({ kept_data: expect.arrayContaining([expect.objectContaining({ environment_id: TRACE.environment, name: 'Kept workspace' })]) });
  await hostKept(page);
  await expect(envRow(page)).toContainText('Kept workspace');
  await expect(envRow(page)).toContainText('2 GiB');
  await expect(page.getByRole('button', { name: 'Delete permanently', exact: true })).toHaveCount(0);
  expect(unsupported).toEqual([]);
});

test('Remove describes a positive window, zero-day deletion and a connected registration truthfully', async ({ page }) => {
  await signIn(page); await page.goto(`/resources/environments/${RUNS_ON.sandbox}`); await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click(); await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click(); await expect(page.getByText(/Its files are kept until/)).toBeVisible(); await expect(page.getByText('Workspace, home, state and private views are included. Mounts are kept separately.')).toBeVisible(); await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await retention({ keep_days: 0 }); await page.reload(); await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click(); await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
  await expect(page.getByText('Deletes the unit and its owned volumes without a recovery window.')).toBeVisible();
  await expect(page.getByText(/Its files are kept until/)).toHaveCount(0);
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`); await page.getByRole('button', { name: 'Actions for Workspace', exact: true }).click(); await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click(); await expect(page.getByText('Only its registration is removed; its files stay on this machine.')).toBeVisible();
});
