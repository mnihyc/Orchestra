/** Recorded agent responsibility, request continuation and guarded mixed restores remain usable by keyboard on a phone. */
import { expect, test, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { World } from '../doubles/world';
import { TRACE } from '../doubles/fixtures';
import { ACME, deny, expectAccessibleSettled, scenario, signIn } from './support';
import type { AuditEntry as TraceEntry } from '../../src/generated/api-types';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41800'}`;
async function control(path: string, body: Record<string, unknown> = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/trace${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(response.ok).toBe(true); return response.status === 204 ? null : response.json() as Promise<unknown>;
}
interface Recorded { records: { entry: TraceEntry }[]; deletion_ids: Record<string, string>; requests: { method: string; path: string; body: Record<string, unknown>; idempotency_key: string | null }[] }
const state = async () => control('/state') as Promise<Recorded>;
async function choose(page: Page, id: string) { const tick = page.locator(`[data-trace-object="${id}"]`).getByRole('checkbox'); await tick.focus(); await page.keyboard.press('Space'); await expect(tick).toBeChecked(); }
test.beforeEach(async () => { await scenario(); });

test('the person entry includes their frozen agent acts and the direct-only choice survives reload', async ({ page }) => {
  await control('/investigation'); await signIn(page); await page.goto('/org/acme/settings/members'); await page.getByRole('button', { name: 'Actions for Dora Developer', exact: true }).click(); await page.getByRole('menuitem', { name: 'What they did here' }).click();
  await expect(page).toHaveURL(/include_agents=true/); await expect(page.getByRole('checkbox', { name: 'By them, and by agents for them' })).toBeChecked();
  await expect(page.getByText('Agent in Thread name recorded then, for Dora Developer', { exact: true })).toBeVisible(); await expect(page.getByText(/for Grace Hopper/)).toHaveCount(0); await expect(page.getByText(/Person not recorded/)).toHaveCount(0);
  await page.getByRole('checkbox', { name: 'By them, and by agents for them' }).uncheck(); await expect(page.getByText('Agent in Thread name recorded then, for Dora Developer', { exact: true })).toHaveCount(0); await page.reload(); await expect(page).toHaveURL(/include_agents=false/); await expect(page.getByRole('checkbox', { name: 'By them, and by agents for them' })).not.toBeChecked();
  await page.goto('/org/acme/settings/audit?kind=changed'); await expect(page.getByText('Agent in Thread name recorded then, person not recorded', { exact: true })).toBeVisible();
});

test('one request continues in its open line and Download preserves every matching exact row and recorded holder option', async ({ page }) => {
  await control('/investigation'); await signIn(page); await page.goto('/org/acme/settings/audit?kind=access&action=permission.added&from=2026-10-01&to=2026-10-13');
  const request = page.locator('[data-trace-request="one-share-request"]'); await expect(request).toHaveCount(1); await expect(page.locator('[data-trace-row]')).toHaveCount(3);
  await request.getByRole('button', { name: 'Show details of Permission added', exact: true }).click(); await expect(request.locator('[data-trace-detail]')).toHaveCount(38); await expect(request.getByText('In a managed environment', { exact: true }).first()).toBeVisible(); await expect(request.getByText('Earlier group name', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: 'Show more', exact: true }).click(); await expect(request.locator('[data-trace-detail]')).toHaveCount(45); await expect(page.locator('[data-trace-request="different-share-request"]')).toHaveCount(1); await expect(page.locator('[data-trace-row]')).toHaveCount(3); await expect(request.getByText(/matching rows?/)).toHaveCount(0);
  await page.getByRole('button', { name: 'Download', exact: true }).click(); const dialog = page.getByRole('dialog'); await dialog.getByRole('combobox', { name: 'Format' }).click(); await page.getByRole('option', { name: 'JSON Lines', exact: true }).click(); const downloaded = page.waitForEvent('download'); await dialog.getByRole('button', { name: 'Download', exact: true }).click();
  const file = await downloaded; const path = test.info().outputPath('audit.jsonl'); await file.saveAs(path); const rows = (await readFile(path, 'utf8')).trim().split('\n').map((line) => JSON.parse(line) as TraceEntry); expect(rows).toHaveLength(47); expect(rows.filter((row) => row.request_id === 'one-share-request')).toHaveLength(45);
});

test('keyboard selection confirms a mixed parent group, document and environment once and follows the stopped job', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted');
  for (const id of [TRACE.project, TRACE.child, TRACE.document, TRACE.environment]) await choose(page, id);
  await page.getByRole('button', { name: 'Bring back', exact: true }).click(); const dialog = page.getByRole('dialog'); await expect(dialog.getByText('Included with Archived experiment; no separate request.', { exact: true })).toBeVisible(); await expect(dialog.getByRole('button', { name: 'Bring back', exact: true })).toBeEnabled(); await expectAccessibleSettled(page, '[role="dialog"]');
  await dialog.getByRole('button', { name: 'Bring back', exact: true }).click(); await expect(dialog.getByText('Brought back Archived experiment · Archived', { exact: true })).toBeVisible(); await expect(dialog.getByText('Brought back Investigation notes · Document', { exact: true })).toBeVisible(); await expect(dialog.getByText('Stopped', { exact: true })).toBeVisible(); await expect(dialog.getByText('Brought back with Archived experiment', { exact: true })).toBeVisible();
  const traffic = await state(); const requests = traffic.requests.filter((row) => row.method === 'POST' && row.path.endsWith('/restore')); expect(requests.map((row) => row.path).sort()).toEqual([`/nodes/${TRACE.project}/restore`, `/documents/${TRACE.document}/restore`, `/environments/${TRACE.environment}/restore`].sort()); expect(requests.every((row) => row.idempotency_key !== null)).toBe(true); expect(requests.find((row) => row.path.startsWith('/nodes'))?.body.expected_deletion_id).toBeTruthy(); expect(traffic.requests.filter((row) => /\/(start|inputs|read|seen|subscription)$/.test(row.path) && row.method !== 'GET')).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('a child alone names its still-deleted parent and sends no implicit parent restore', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted'); await choose(page, TRACE.child); await page.getByRole('button', { name: 'Bring back', exact: true }).click(); const dialog = page.getByRole('dialog'); await expect(dialog.getByText('Bring back Archived experiment first. Its restoration needs separate authority.', { exact: true })).toBeVisible(); await expect(dialog.getByRole('button', { name: 'Bring back', exact: true })).toBeDisabled(); expect((await state()).requests.filter((row) => row.method === 'POST' && row.path.endsWith('/restore'))).toEqual([]);
  await dialog.getByRole('button', { name: 'Include Archived experiment', exact: true }).click(); await expect(dialog.getByText('Included with Archived experiment; no separate request.', { exact: true })).toBeVisible(); await dialog.getByRole('button', { name: 'Bring back', exact: true }).click(); await expect(dialog.getByText('Brought back with Archived experiment', { exact: true })).toBeVisible(); expect((await state()).requests.filter((row) => row.method === 'POST' && row.path.endsWith('/restore')).map((row) => row.path)).toEqual([`/nodes/${TRACE.project}/restore`]);
});

test('a newer deletion at the same time, lost document authority and a winning purge do not roll back a successful node', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted'); for (const id of [TRACE.project, TRACE.thread, TRACE.document, TRACE.environment]) await choose(page, id);
  await page.getByRole('button', { name: 'Bring back', exact: true }).click(); const dialog = page.getByRole('dialog'); await expect(dialog.getByRole('button', { name: 'Bring back', exact: true })).toBeEnabled();
  await control('/deletion', { id: TRACE.thread, redelete: true }); await deny('ada', [`doc.manage@${TRACE.document}`]); await control('/retention', { environment: TRACE.environment, phase: 'purging', hold_jobs: true });
  await dialog.getByRole('button', { name: 'Bring back', exact: true }).click(); await expect(dialog.getByText('Brought back Archived experiment · Archived', { exact: true })).toBeVisible(); for (const id of [TRACE.thread, TRACE.document, TRACE.environment]) await expect(dialog.locator(`[data-restore-result="${id}"]`).getByRole('alert')).toBeVisible(); await expect(dialog.getByText('Stopped', { exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Close results', exact: true }).click(); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); const held = await state(); const records = held.records.map((row) => row.entry).filter((row) => row.object.id === TRACE.thread && row.kind === 'deleted'); const old = records.find((row) => row.event_deletion_id !== held.deletion_ids[TRACE.thread])!; const newer = records.find((row) => row.event_deletion_id === held.deletion_ids[TRACE.thread])!; expect(old.at).toBe(newer.at); await expect(page.locator(`[data-trace-row="${old.id}"]`).getByRole('checkbox')).toHaveCount(0); await expect(page.locator(`[data-trace-row="${newer.id}"]`).getByRole('checkbox')).toBeVisible();
});

test('refreshing an old selection shows the changed deletion before confirmation; legacy and purged events have no tick', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted'); await choose(page, TRACE.thread); await control('/deletion', { id: TRACE.thread, redelete: true }); await page.getByRole('button', { name: 'Bring back', exact: true }).click(); const dialog = page.getByRole('dialog'); await expect(dialog.getByText('The deletion changed. This old selection cannot bring it back.', { exact: true })).toBeVisible(); await expect(dialog.getByRole('button', { name: 'Bring back', exact: true })).toBeDisabled(); await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await control('/deletion', { id: TRACE.document, unknown: true }); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(page.locator(`[data-trace-object="${TRACE.document}"]`).getByRole('checkbox')).toHaveCount(0); await expect(page.locator(`[data-trace-object="${TRACE.purged}"]`).getByRole('checkbox')).toHaveCount(0);
});

test('a changed removal revision refuses only the environment and a document success stays visible', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted'); await choose(page, TRACE.document); await choose(page, TRACE.environment); await page.getByRole('button', { name: 'Bring back', exact: true }).click(); const dialog = page.getByRole('dialog'); await expect(dialog.getByRole('button', { name: 'Bring back', exact: true })).toBeEnabled(); await control('/retention', { environment: TRACE.environment, stale: true }); await dialog.getByRole('button', { name: 'Bring back', exact: true }).click(); await expect(dialog.getByText('Brought back Investigation notes · Document', { exact: true })).toBeVisible(); await expect(dialog.locator(`[data-restore-result="${TRACE.environment}"]`).getByRole('alert')).toBeVisible(); await expect(dialog.getByText('Stopped', { exact: true })).toHaveCount(0);
});

test('revoked trail authority clears the phone selection along with the rows and Download', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted'); await choose(page, TRACE.thread); await deny('ada', ['audit.view']); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(page.getByText('You no longer have access to this trail')).toBeVisible(); await expect(page.getByRole('button', { name: 'Bring back', exact: true })).toHaveCount(0); await expect(page.locator('[data-trace-row]')).toHaveCount(0); await expect(page.getByRole('button', { name: 'Download', exact: true })).toHaveCount(0); expect((await state()).requests.filter((row) => row.path === `/nodes/${ACME.org}/restore`)).toEqual([]);
});

test.describe('the trace-shaped world follows the same guards', () => {
  test('filters two actors and frozen agent responsibility before paging and export; unknown responsibility stays unknown', async () => {
    const world = new World(); const ada = world.people.get('ada')!; const dora = world.places.userOfName('dora'); world.trace.control('/investigation', {});
    const direct = await world.trace.handle(ada, 'GET', `/nodes/${ACME.org}/audit`, new URLSearchParams({ actor: dora.id, kind: 'changed', include_agents: 'false' }), {});
    expect((direct?.body as { items: TraceEntry[] }).items).toEqual([]);
    const included = await world.trace.handle(ada, 'GET', `/nodes/${ACME.org}/audit`, new URLSearchParams({ actor: dora.id, kind: 'changed', include_agents: 'true' }), {});
    expect((included?.body as { items: TraceEntry[] }).items.map((row) => row.request_id)).toEqual(['turn-for-dora']);
    const exported = await world.trace.handle(ada, 'GET', `/nodes/${ACME.org}/audit/export`, new URLSearchParams({ actor: dora.id, include_agents: 'true', kind: 'access', from: '2026-10-01T00:00:00Z', to: '2026-10-13T00:00:00Z', format: 'jsonl' }), {});
    const exact = new TextDecoder().decode(exported?.bytes).trim().split('\n').map((line) => JSON.parse(line) as TraceEntry);
    expect(exact).toHaveLength(46); expect(exact.filter((row) => row.request_id === 'one-share-request')).toHaveLength(45);
  });
  test('refuses a child alone and a same-time newer deletion; a parent restores its selected group once without any turn', async () => {
    const world = new World(); const ada = world.people.get('ada')!; const original = world.trace.deletionIds.get(TRACE.thread)!;
    await expect(world.trace.handle(ada, 'POST', `/nodes/${TRACE.child}/restore`, new URLSearchParams(), { expected_deletion_id: world.trace.deletionIds.get(TRACE.child) }, 'child-act')).rejects.toMatchObject({ answer: { status: 409, body: { code: 'node.restore_parent_first' } } });
    await world.trace.handle(ada, 'POST', `/nodes/${TRACE.thread}/restore`, new URLSearchParams(), { expected_deletion_id: original }, 'restore-first');
    world.trace.control('/deletion', { id: TRACE.thread, redelete: true });
    expect(world.trace.retained.get(TRACE.thread)?.inspection.deleted_at).toBe('2026-10-12T12:00:00.000Z');
    expect(world.trace.deletionIds.get(TRACE.thread)).not.toBe(original);
    await expect(world.trace.handle(ada, 'POST', `/nodes/${TRACE.thread}/restore`, new URLSearchParams(), { expected_deletion_id: original }, 'stale-act')).rejects.toMatchObject({ answer: { status: 409, body: { code: 'conflict.stale_revision' } } });
    const group = world.trace.deletionIds.get(TRACE.project)!;
    await world.trace.handle(ada, 'POST', `/nodes/${TRACE.project}/restore`, new URLSearchParams(), { expected_deletion_id: group }, 'parent-act');
    await world.trace.handle(ada, 'POST', `/nodes/${TRACE.project}/restore`, new URLSearchParams(), { expected_deletion_id: group }, 'parent-act');
    expect(world.trace.retained.has(TRACE.child)).toBe(false); expect(world.trace.records.filter(({ entry }) => entry.action === 'node.restored' && entry.object.id === TRACE.project)).toHaveLength(1);
    expect((world.threads.snapshot(TRACE.thread) as { turns: { state: string }[] }).turns.filter((turn) => turn.state === 'running')).toEqual([]);
  });
});
