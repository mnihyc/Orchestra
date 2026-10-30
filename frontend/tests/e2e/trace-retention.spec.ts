/** Kept files, actual stopped restoration jobs, permanent removal races and truthful failure/removal wording. */
import { expect, test, type Page } from '@playwright/test';
import { RUNS_ON, TRACE } from '../doubles/fixtures';
import { ACME, PEOPLE, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41800'}`;
async function retention(body: Record<string, unknown>) { const response = await fetch(`${DOUBLE}/__double/trace/retention`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }); expect(response.ok).toBe(true); }
async function state() { return (await fetch(`${DOUBLE}/__double/trace/state`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).json() as Promise<{ requests: { method: string; path: string; query: Record<string, string>; body: Record<string, unknown> }[] }>; }
async function deleted(page: Page) { await page.goto(`/project/${ACME.web}`); await page.getByRole('button', { name: /^Actions for Web(?: · .+)?$/, exact: true }).first().click(); await page.getByRole('menuitem', { name: 'Recently deleted…', exact: true }).click(); await expect(page.getByRole('dialog', { name: /^Recently deleted in the project Web, in / })).toBeVisible(); }
const envRow = (page: Page, id: string = TRACE.environment) => page.locator(`[data-kept-environment="${id}"]`);
test.beforeEach(async () => { await scenario(); });

test('Recently deleted includes guarded nodes, documents, measured and unknown environment size, host, storage and phase', async ({ page }) => {
  await signIn(page); await deleted(page); await expect(page.getByText('Archived experiment', { exact: true })).toBeVisible(); await expect(page.locator(`[data-kept-document="${TRACE.document}"]`)).toBeVisible(); await expect(envRow(page)).toContainText('2 GiB'); await expect(envRow(page)).toContainText('host-1'); await expect(envRow(page)).toContainText('Kept'); await expect(envRow(page, TRACE.unknownEnvironment)).toContainText('Unknown');
  await envRow(page).getByRole('link', { name: 'Look at it' }).click(); await expect(page).toHaveURL(`/inspect/environment/${TRACE.environment}`); await expect(page.getByText('Read-only inspection. Reading changes no read marks or notifications.')).toBeVisible(); await expect(page.getByRole('button', { name: 'Start', exact: true })).toHaveCount(0); const traffic = await state(); expect(traffic.requests.filter((request) => request.path.endsWith('/start') || request.path.endsWith('/files'))).toEqual([]);
});

test('Bring it back shows its real job and only reports Stopped after the server returns it', async ({ page }) => {
  await signIn(page); await deleted(page); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(page.getByText(/^Job /)).toBeVisible(); await expect(page.getByText('Stopped', { exact: true })).toBeVisible(); await expect(page.getByText('Restoration returns Stopped. Starting it and restoring its thread are separate acts.').first()).toBeVisible(); await expect(page.getByRole('link', { name: 'Open environment' })).toBeVisible();
  const traffic = await state(); const request = traffic.requests.find((one) => one.path === `/environments/${TRACE.environment}/restore`)!; expect(request.body.expected_revision).toBe(1); expect(traffic.requests.filter((one) => one.path.endsWith('/start') || one.path === `/nodes/${TRACE.thread}/restore`)).toEqual([]);
});

test('an environment can also be restored from its separately authorized audit row', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted&action=environment.deleted'); const row = page.locator(`[data-trace-object="${TRACE.environment}"]`); await row.getByRole('button', { name: 'Bring it back', exact: true }).click(); const dialog = page.getByRole('dialog'); await dialog.getByRole('button', { name: 'Bring back', exact: true }).click(); await expect(dialog.getByText(/^Job /)).toBeVisible(); await expect(dialog.getByText('Stopped', { exact: true })).toBeVisible();
});

test('an environment manager who is not a home admin may restore but has no early purge act', async ({ page }) => {
  await signIn(page, PEOPLE.dora); await deleted(page); await expect(envRow(page).getByRole('button', { name: 'Bring it back' })).toBeVisible(); await expect(page.getByRole('button', { name: 'Remove for good', exact: true })).toHaveCount(0);
  // A direct permanent-removal request still uses intrinsic home authority.
  const auth = await page.request.post('/api/v1/auth/login', { data: { login: PEOPLE.dora.login, password: PEOPLE.dora.password } }); const grant = await auth.json() as { access_token: string }; const purge = await page.request.delete(`/api/v1/environments/${TRACE.environment}/purge?confirm=Kept+workspace&expected_revision=1`, { headers: { Authorization: `Bearer ${grant.access_token}` } }); expect(purge.status()).toBe(403);
});

test('Remove for good confirms the name and actual size; a purge that wins refuses stale restoration', async ({ page }) => {
  await signIn(page); await deleted(page); await envRow(page).getByRole('button', { name: 'Remove for good', exact: true }).click(); const dialog = page.getByRole('dialog', { name: 'Remove Kept workspace for good' }); await expect(dialog.getByRole('button', { name: 'Remove for good', exact: true })).toBeDisabled(); await dialog.getByLabel('Type Kept workspace to confirm').fill('Kept workspace'); await dialog.getByRole('checkbox').check(); await dialog.getByRole('button', { name: 'Remove for good', exact: true }).click(); await expect(page.getByText(/^Job /)).toBeVisible();
  const traffic = await state(); const purge = traffic.requests.find((one) => one.path.endsWith('/purge'))!; expect(purge.query.confirm).toBe('Kept workspace'); expect(purge.query.expected_revision).toBe('1');
  await expect(page.getByText('Committed · succeeded', { exact: true })).toBeVisible(); await page.goto(`/inspect/environment/${TRACE.environment}`); await expect(page.getByText('Could not look at this deleted thing')).toBeVisible();
});

test('a purge already running and changed authority refuse a stale Bring it back without inventing an empty replacement', async ({ page }) => {
  await signIn(page); await deleted(page); await retention({ environment: TRACE.environment, phase: 'purging', hold_jobs: true }); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(envRow(page).getByRole('button', { name: 'Bring it back', exact: true })).toHaveCount(0); await expect(page.getByText(/^Job /)).toBeVisible(); await expect(page.getByText('Stopped', { exact: true })).toHaveCount(0);
  await retention({ username: 'ada', can_restore: false, can_purge: false }); await page.reload(); await deleted(page); await expect(page.getByRole('button', { name: 'Remove for good', exact: true })).toHaveCount(0);
});

test('a missing host keeps the deadline and failure job on the host and in Inbox', async ({ page }) => {
  await signIn(page); await retention({ failure: 'host-1 is not reachable; the kept volumes remain.' }); await deleted(page); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(page.getByText(/The job has not completed: host-1 is not reachable/)).toBeVisible(); await expect(envRow(page)).toContainText('Kept until');
  await page.goto('/inbox'); await page.getByRole('link', { name: /Kept files need attention/ }).click(); await expect(page).toHaveURL(/\/resources\/hosts\//); await expect(page.getByRole('heading', { name: 'Kept after removal' })).toBeVisible(); await expect(envRow(page).getByText(/Job /)).toContainText('failed'); await expect(envRow(page)).toContainText('Kept until'); await expect(page.getByRole('button', { name: 'Remove for good', exact: true })).toHaveCount(0);
});

test('a stale removal revision and a full disk keep the files deleted with the original deadline', async ({ page }) => {
  await signIn(page); await deleted(page); await retention({ environment: TRACE.environment, stale: true }); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(envRow(page).getByRole('alert')).toBeVisible(); await expect(page.getByText('Stopped', { exact: true })).toHaveCount(0);
  await retention({ failure: 'The host has no free disk space. Kept volumes remain until their original day.' }); await envRow(page).getByRole('button', { name: 'Bring it back', exact: true }).click(); await expect(page.getByText(/The job has not completed: The host has no free disk space/)).toBeVisible(); await expect(envRow(page)).toContainText('Kept until'); await expect(page.getByText('Stopped', { exact: true })).toHaveCount(0);
});

test('a host manager alone reads safe retained facts and has no restore or early purge control', async ({ page }) => {
  await retention({ host_manager: 'vera' }); await signIn(page, PEOPLE.vera); await page.goto('/resources/hosts/00000000-0000-4000-8025-000000000001'); await expect(page.getByRole('heading', { name: 'Kept after removal' })).toBeVisible(); await expect(envRow(page)).toContainText('Private environment'); await expect(page.getByText('Kept workspace', { exact: true })).toHaveCount(0); await expect(page.getByRole('button', { name: 'Remove for good', exact: true })).toHaveCount(0); await expect(page.getByRole('button', { name: 'Bring it back', exact: true })).toHaveCount(0);
});

test('QS-013 the host detail uses its served kept_data and makes no speculative retained request', async ({ page }) => {
  await signIn(page);
  const unsupported: string[] = [];
  page.on('request', (request) => { if (/\/hosts\/[^/]+\/retained$/.test(new URL(request.url()).pathname)) unsupported.push(request.url()); });
  const reply = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/hosts/00000000-0000-4000-8025-000000000001');
  await page.goto('/resources/hosts/00000000-0000-4000-8025-000000000001');
  const wire: unknown = await (await reply).json();
  expect(wire).toMatchObject({ kept_data: expect.arrayContaining([expect.objectContaining({ environment_id: TRACE.environment, name: 'Kept workspace' })]) });
  await expect(envRow(page)).toContainText('Kept workspace');
  await expect(envRow(page)).toContainText('2 GiB');
  await expect(page.getByRole('button', { name: 'Remove for good', exact: true })).toHaveCount(0);
  expect(unsupported).toEqual([]);
});

test('Remove describes a positive window, zero-day deletion and a connected registration truthfully', async ({ page }) => {
  await signIn(page); await page.goto(`/resources/environments/${RUNS_ON.sandbox}`); await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click(); await page.getByRole('menuitem', { name: 'Remove…', exact: true }).click(); await expect(page.getByText(/Its files are kept until/)).toBeVisible(); await expect(page.getByText('Workspace, home, state and private views are included. Mounts are kept separately.')).toBeVisible(); await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await retention({ keep_days: 0 }); await page.reload(); await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click(); await page.getByRole('menuitem', { name: 'Remove…', exact: true }).click(); await expect(page.getByText('The files are removed for good when the unit is removed.')).toBeVisible(); await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`); await page.getByRole('button', { name: 'Actions for Workspace', exact: true }).click(); await page.getByRole('menuitem', { name: 'Remove…', exact: true }).click(); await expect(page.getByText('Only its registration is removed; its files stay on this machine.')).toBeVisible();
});
