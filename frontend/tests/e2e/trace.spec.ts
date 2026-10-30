/** Person entries, scoped trails, filtering, keyboard details and complete downloads in the shared world. */
import { expect, test, type Page } from '@playwright/test';
import { id } from '../doubles/fixtures';
import english from '../../src/i18n/en.json' with { type: 'json' };
import { openShare } from './give-support';
import { ACME, PEOPLE, deny, scenario, signIn } from './support';

const removedPermission = english.trace.actionVerbs.removed.replace('{{thing}}', english.trace.actionThings.permission);
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41800'}`;
async function configure(body: Record<string, unknown>) { const response = await fetch(`${DOUBLE}/__double/trace/configure`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }); expect(response.ok).toBe(true); }
async function state() { return (await fetch(`${DOUBLE}/__double/trace/state`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).json() as Promise<{ requests: { path: string; query: Record<string, string> }[] }>; }
test.beforeEach(async () => { await scenario(); });

async function systemPage(page: Page, path: '/admin/users' | '/admin/audit') {
  await page.goto(path);
  await expect(page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: Off', exact: true })).not.toBeChecked();
  await page.getByRole('button', { name: 'Turn on As admin', exact: true }).click();
  await expect(page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: On', exact: true })).toBeChecked();
  await page.goto(path);
}


test('Members opens this org with that person, and reload keeps every filter', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/members');
  await page.getByRole('button', { name: 'Actions for Dora Developer', exact: true }).click(); await page.getByRole('menuitem', { name: 'What they did here' }).click();
  await expect(page).toHaveURL(/\/org\/acme\/settings\/audit\?actor=/); await expect(page.getByText('Acts by Dora Developer')).toBeVisible();
  await page.getByRole('combobox', { name: 'Kind of act', exact: true }).click(); await page.getByRole('option', { name: 'Access', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Exact action', exact: true })).toBeVisible();
  await page.getByRole('combobox', { name: 'Exact action', exact: true }).fill('permission.removed'); await page.getByRole('option', { name: removedPermission, exact: true }).click();
  await page.getByLabel('From', { exact: true }).fill('2026-10-01'); await page.getByLabel('To', { exact: true }).fill('2026-10-13');
  await page.reload(); await expect(page.getByRole('combobox', { name: 'Exact action' })).toHaveValue(removedPermission); await expect(page.getByLabel('From', { exact: true })).toHaveValue('2026-10-01');
  const asked = (await state()).requests.filter((one) => one.path.endsWith('/audit')).at(-1)!; expect(asked.path).toBe(`/nodes/${ACME.org}/audit`); expect(asked.query.kind).toBe('access'); expect(asked.query.action).toBe('permission.removed'); expect(asked.query.include_agents).toBe('true');
});

test('People and a person card keep the selected project and topic', async ({ page }) => {
  await signIn(page); await openShare(page, `/project/${ACME.web}`, 'Web');
  await page.getByRole('tab', { name: 'People', exact: true }).click();
  await page.getByRole('button', { name: /Admins of Acme/ }).click();
  const person = page.getByRole('button', { name: 'Ada Lovelace', exact: true }).first(); await person.click();
  await page.getByRole('link', { name: 'What they did here', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/project/${ACME.web}/settings\\?.*section=audit`)); await expect(page.getByRole('heading', { name: 'Audit trail · Web' })).toBeVisible();
  await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=audit`); await expect(page.getByRole('heading', { name: 'Audit trail · Fix login' })).toBeVisible(); expect((await state()).requests.filter((one) => one.path.endsWith('/audit')).at(-1)?.path).toBe(`/nodes/${ACME.fixLogin}/audit`);
});

test('Management Users opens only the system trail and authority is rechecked', async ({ page }) => {
  await signIn(page); await systemPage(page, '/admin/users'); await page.getByRole('button', { name: 'Actions for Dora Developer', exact: true }).click(); await page.getByRole('menuitem', { name: 'What they did here' }).click();
  await expect(page).toHaveURL(/\/admin\/audit\?actor=/);
  // Management owns the heading; its embedded trail expresses the actor in the Person control.
  await expect(page.getByRole('combobox', { name: english.trace.person, exact: true })).toHaveValue('Dora Developer');
  await expect(page.getByRole('checkbox', { name: english.trace.includeAgents, exact: true })).toBeChecked();
  const actor = id(1, 9);
  expect(new URL(page.url()).searchParams.get('actor')).toBe(actor);
  expect(new URL(page.url()).searchParams.get('include_agents')).toBe('true');
  const asked = (await state()).requests.filter((one) => one.path.endsWith('/audit')).at(-1);
  expect(asked?.path).toBe('/admin/audit'); expect(asked?.query).toMatchObject({ actor, include_agents: 'true' });
  await page.reload();
  await expect(page.getByRole('combobox', { name: english.trace.person, exact: true })).toHaveValue('Dora Developer');
  await expect(page.getByRole('checkbox', { name: english.trace.includeAgents, exact: true })).toBeChecked();
  expect((await state()).requests.filter((one) => one.path.endsWith('/audit')).at(-1)?.query).toMatchObject({ actor, include_agents: 'true' });
});

test('without audit authority the person menu has no entry and a direct URL is refused', async ({ page }) => {
  await signIn(page, PEOPLE.vera); await page.goto('/org/acme/settings/members'); await page.getByRole('button', { name: 'Actions for Dora Developer', exact: true }).click(); await expect(page.getByRole('menuitem', { name: 'What they did here' })).toHaveCount(0); await page.keyboard.press('Escape');
  await page.goto('/org/acme/settings/audit'); await expect(page.getByText('You no longer have access to this trail')).toBeVisible(); await expect(page.locator('[data-trace-row]:visible')).toHaveCount(0); await expect(page.getByRole('button', { name: 'Download', exact: true })).toHaveCount(0);
});

test('day and kind continue across pages, details open with Enter and close with Escape', async ({ page }) => {
  await signIn(page); await systemPage(page, '/admin/audit'); await expect(page.locator('[data-trace-row]:visible')).toHaveCount(40);
  await page.getByRole('button', { name: 'Show more', exact: true }).click(); await expect(page.locator('[data-trace-row]:visible')).toHaveCount(80);
  const first = page.locator('[data-trace-row]:visible').first().getByRole('button').first(); await first.focus(); await page.keyboard.press('Enter'); await expect(first).toHaveAttribute('aria-expanded', 'true'); await page.keyboard.press('Escape'); await expect(first).toHaveAttribute('aria-expanded', 'false');
  const grouped = page.getByRole('combobox', { name: english.trace.group, exact: true });
  await grouped.click(); await page.getByRole('option', { name: english.choiceCatalog.traceGroup.kind.name, exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Access', exact: true })).toHaveCount(1);
  await expect(page).toHaveURL(/(?:\?|&)group=kind(?:&|$)/);
  await page.reload(); await expect(grouped).toContainText(english.choiceCatalog.traceGroup.kind.name);
  expect(new URL(page.url()).searchParams.get('group')).toBe('kind');
});

test('Download uses the current filters and an incomplete file is never saved', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?action=permission.removed&kind=access&from=2026-10-01&to=2026-10-13'); await page.getByRole('button', { name: 'Download', exact: true }).click();
  const downloaded = page.waitForEvent('download'); await page.getByRole('dialog').getByRole('button', { name: 'Download', exact: true }).click(); const file = await downloaded; expect(file.suggestedFilename()).toBe('audit.csv');
  const asked = (await state()).requests.findLast((request) => request.path.endsWith('/export'))!; expect(asked.query).toMatchObject({ kind: 'access', action: 'permission.removed', from: '2026-10-01T00:00:00.000Z', to: '2026-10-13T23:59:59.999Z', format: 'csv' });
  await configure({ incomplete: true }); let files = 0; page.on('download', () => { files += 1; }); await page.getByRole('button', { name: 'Download', exact: true }).click(); await page.getByRole('dialog').getByRole('button', { name: 'Download', exact: true }).click(); await expect(page.getByText('The download was not completed. No partial file was saved.')).toBeVisible(); expect(files).toBe(0);
});

test('empty, failed Retry and revoked authority remove stale rows and Download', async ({ page }) => {
  await signIn(page); await configure({ empty: true }); await page.goto('/org/acme/settings/audit'); await expect(page.getByText('No recorded acts', { exact: true })).toBeVisible();
  await page.getByLabel('From', { exact: true }).fill('2026-10-01'); await expect(page.getByText('No acts match', { exact: true })).toBeVisible();
  await configure({ failing: true }); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeVisible(); await configure({ failing: false }); await page.getByRole('button', { name: 'Try again', exact: true }).click(); await expect(page.getByText('No acts match', { exact: true })).toBeVisible();
  await deny('ada', ['audit.view']); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(page.getByText('You no longer have access to this trail')).toBeVisible(); await expect(page.getByRole('button', { name: 'Download', exact: true })).toHaveCount(0);
});
