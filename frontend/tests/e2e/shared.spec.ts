/** Shares outside a browsable tree have a usable entrance, private ancestor names and recoverable list states. */
import { expect, test } from '@playwright/test';
import type { Node } from '../../src/generated/api-types';
import { signInWithProjectionReader } from './projection-support.ts';
import { ACME, scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

test('the rail opens Shared with me and the list keeps a private ancestor unnamed', async ({ page }) => {
  const read = await signInWithProjectionReader(page);
  const node = await read<Node>(`/api/v1/nodes/${ACME.mainThread}`);
  const shared: Node = { ...node, name: 'Just this thread', path: [{ id: ACME.org, kind: 'org', name: null, redacted: true }, { id: node.id, kind: node.kind, name: 'Just this thread', redacted: false }] };
  let rows = [shared];
  await page.route('**/api/v1/shared', async (route) => { await route.fulfill({ json: { items: rows, partial: false, next_cursor: null } }); });
  await page.getByRole('link', { name: 'Shared with me', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'Shared with me', exact: true })).toBeVisible();
  const table = page.getByRole('table', { name: 'Shared with me', exact: true });
  await expect(table.getByRole('link', { name: 'Just this thread', exact: true })).toHaveAttribute('href', `/thread/${ACME.mainThread}`);
  await expect(table.getByRole('link', { name: 'Acme', exact: true })).toHaveCount(0);
  await expect(table).toContainText('Private');
  await page.getByRole('combobox', { name: 'Kind', exact: true }).click();
  await page.getByRole('option', { name: 'Project', exact: true }).click();
  await expect(page.getByText('No shared thing matches', { exact: true })).toBeVisible();
  await page.getByRole('combobox', { name: 'Kind', exact: true }).click();
  await page.getByRole('option', { name: 'Thread', exact: true }).click();
  await expect(table).toBeVisible();
  await page.getByRole('searchbox', { name: 'Filter this page', exact: true }).fill('absent name');
  await expect(page.getByText('No shared thing matches', { exact: true })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Filter this page', exact: true }).fill('Just this');
  await expect(table).toBeVisible();
  rows = [];
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByText('Nothing shared here yet', { exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Just this thread', exact: true })).toHaveCount(0);
});

test('a failed shared-list read is an error with retry and never an empty success', async ({ page }) => {
  let failed = true;
  await page.route('**/api/v1/shared', async (route) => { await route.fulfill(failed ? { status: 503, json: { code: 'internal', message: 'Temporarily unavailable.' } } : { json: { items: [], partial: false, next_cursor: null } }); });
  await signIn(page);
  await page.goto('/shared');
  await expect(page.getByText('Nothing shared here yet', { exact: true })).toHaveCount(0);
  const retry = page.getByRole('button', { name: /Try again/i }).first();
  await expect(retry).toBeVisible();
  failed = false;
  await retry.click();
  await expect(page.getByText('Nothing shared here yet', { exact: true })).toBeVisible();
});

test('an empty shared candidate page exposes its continuation without eagerly reading it', async ({ page }) => {
  const read = await signInWithProjectionReader(page);
  const node = await read<Node>(`/api/v1/nodes/${ACME.web}`);
  const cursors: (string | null)[] = [];
  await page.route('**/api/v1/shared*', async (route) => {
    const cursor = new URL(route.request().url()).searchParams.get('cursor');
    cursors.push(cursor);
    await route.fulfill({ json: cursor === 'later'
      ? { items: [{ ...node, name: 'Later readable share' }], partial: false, next_cursor: null }
      : { items: [], partial: true, next_cursor: 'later' } });
  });
  await page.goto('/shared');
  expect(cursors).not.toContain('later');
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  const table = page.getByRole('table', { name: 'Shared with me', exact: true });
  await expect(table.getByRole('link', { name: 'Later readable share', exact: true })).toBeVisible();
  expect(cursors).toContain('later');
  await expect(page.getByText('Nothing shared here yet', { exact: true })).toHaveCount(0);
});

test('the phone’s navigation menu has one Shared with me entrance', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation', exact: true });
  await expect(drawer).toBeVisible();
  await expect(drawer.getByRole('combobox', { name: 'Your orgs', exact: true })).toBeVisible();
  await drawer.getByRole('button', { name: 'Other places', exact: true }).click();
  const entry = page.getByRole('menuitem', { name: 'Shared with me', exact: true });
  await expect(entry).toHaveCount(1);
  await entry.click();
  await expect(page).toHaveURL(/\/shared\/?$/);
  await expect(page.getByRole('heading', { name: 'Shared with me', exact: true })).toBeVisible();
});
