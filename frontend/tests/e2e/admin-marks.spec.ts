/** Browser proof of the shared presentation contract; native scope/mark equality is checked by the API tests. */
import { expect, test } from '@playwright/test';
import type { Environment } from '../../src/generated/api-types.ts';
import { inventoryRow } from '../doubles/everything-row.ts';
import { asAdmin } from './admin-mode-support.ts';
import { scenario, signIn } from './support.ts';

test('only the top bar holds the switch; native-only rows carry one mark and every owner fits a phone', async ({ page }) => {
  await scenario();
  let enabled = false;
  let own: Environment | undefined;
  const beta = { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01', kind: 'org' as const, name: 'Beta', redacted: false };
  const betaReach = { place: beta, private: false };
  const otherId = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b02';
  // Only the three typed answers under review are controlled. Real layout, navigation, cache
  // rebasing, commands and the global/mirror controls remain the application's own.
  await page.route('**/api/v1/me/admin-mode', async (route) => {
    if (route.request().method() === 'PUT') {
      const body: unknown = route.request().postDataJSON();
      if (typeof body !== 'object' || body === null || !('as_admin' in body) || typeof body.as_admin !== 'boolean') throw new Error('Wrong session-mode body');
      enabled = body.as_admin;
    }
    await route.fulfill({ json: { available: true, as_admin: enabled } });
  });
  await page.route(/\/api\/v1\/environments(?:\?|$)/, async (route) => {
    const reply = await route.fetch();
    const body: unknown = await reply.json();
    const items = Array.isArray(body) ? body : typeof body === 'object' && body !== null && 'items' in body && Array.isArray(body.items) ? body.items : [];
    // These are complete real fixture records, with only the reviewed reach/identity facts varied.
    const supplied = items as Environment[];
    own ??= supplied.find((row) => typeof row.name === 'string' && 'access' in row);
    if (own === undefined) throw new Error('The development world has no complete environment fixture');
    const ordinary = { ...own, name: 'My environment', access: { ...own.access, admin_access: null, participating: true, admin_only: [] } };
    const elevated = { ...own, id: otherId, name: 'Other org environment', home: beta, org_id: beta.id,
      access: { ...own.access, admin_access: betaReach, admin_place: beta, participating: false, admin_only: ['environment.view', 'environment.manage'] } };
    await route.fulfill({ json: enabled ? [ordinary, elevated] : [ordinary] });
  });
  await page.route(/\/api\/v1\/everything(?:\?|$)/, async (route) => {
    const row = inventoryRow({ id: otherId, kind: 'environment', name: 'Other org environment', parent_id: null,
      href: `/resources/environments/${otherId}`, owner: own?.owner ?? null, place_label: 'Beta', admin_access: betaReach });
    const focused = new URL(route.request().url()).searchParams.get('participating') === 'true';
    await route.fulfill({ json: { items: enabled && !focused ? [row] : [], next_cursor: null } });
  });
  await signIn(page);
  await page.goto('/resources/environments');
  const table = page.getByRole('table', { name: 'Environments' });
  await expect(table.getByRole('link', { name: 'My environment', exact: true })).toBeVisible();
  await expect(table.getByRole('link', { name: 'Other org environment', exact: true })).toHaveCount(0);
  await expect(page.locator('[data-resource-navigation]').getByRole('switch', { name: /^As admin:/ })).toHaveCount(0);
  const control = page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: /^As admin: (Off|On)$/ });
  await expect(page.getByRole('switch', { name: /^As admin: (Off|On)$/ })).toHaveCount(1);
  await control.click();
  await expect(control).toHaveAttribute('data-as-admin', 'true');
  await expect(control).toBeChecked();
  const ownRow = table.getByRole('row').filter({ has: page.getByRole('link', { name: 'My environment', exact: true }) });
  const otherRow = table.getByRole('row').filter({ has: page.getByRole('link', { name: 'Other org environment', exact: true }) });
  await expect(otherRow.getByRole('img', { name: 'You see this as an admin of Beta', exact: true })).toBeVisible();
  await expect(ownRow.locator('[data-as-admin-mark]')).toHaveCount(0);
  await page.setViewportSize({ width: 400, height: 820 });
  await expect(otherRow.locator('[data-resource-owner]')).toBeVisible();
  await expect(ownRow.locator('[data-resource-owner]')).toBeVisible();
  await page.goto('/everything?q=Other%20org%20environment');
  const everything = page.getByRole('treegrid');
  await expect(everything.getByRole('link', { name: 'Other org environment', exact: true })).toBeVisible();
  await expect(everything.getByRole('img', { name: 'You see this as an admin of Beta', exact: true })).toBeVisible();
  const filtered = page.waitForRequest((request) => new URL(request.url()).pathname.endsWith('/everything') && new URL(request.url()).searchParams.get('admin_access') === 'true');
  await page.getByRole('checkbox', { name: 'As admin', exact: true }).click();
  await filtered;
  await expect(everything.getByRole('link', { name: 'Other org environment', exact: true })).toBeVisible();
  await asAdmin(page, false);
  await expect(everything.getByRole('link', { name: 'Other org environment', exact: true })).toHaveCount(0);
  await page.goto('/resources/environments');
  await expect(table.getByRole('link', { name: 'Other org environment', exact: true })).toHaveCount(0);
  await expect(control).toHaveAttribute('data-as-admin', 'false');
});
