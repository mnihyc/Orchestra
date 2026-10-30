/** Members and administrators use one complete hierarchy with deliberate columns, safe paths and explicit scope. */
import { expect, test } from '@playwright/test';

import { inventoryRow } from '../doubles/everything-row.ts';
import { ACME, id } from '../doubles/fixtures.ts';
import { asAdmin, signInForManagement } from './admin-mode-support.ts';
import { PEOPLE, expectAccessible, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test.beforeEach(async () => { await scenario(); });

async function inventory(username: string, rows: ReturnType<typeof inventoryRow>[]): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/everything/rows`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username, rows }) });
  if (!response.ok) throw new Error(`The inventory scenario was refused: ${response.status}`);
}

test('Everything leads with its list and opens one bounded Filters popup at desktop and phone widths', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/everything');
    const main = page.locator('[data-main-area]');
    await expect(main.getByRole('treegrid', { name: 'Everything I can see' })).toBeVisible();
    await expect(main.getByRole('heading', { name: 'Everything', exact: true, level: 1 })).toHaveCount(1);
    await expect(main.getByText('Every place and thing you can see, with the details you choose.', { exact: true })).toHaveCount(0);
    await expect(main.getByText('Use Up and Down to move, Right and Left to expand or collapse, and Enter to open.', { exact: true })).toHaveCount(0);
    await expect(main.getByRole('searchbox', { name: 'Search by name', exact: true })).toHaveCount(1);
    await expect(main.getByRole('combobox')).toHaveCount(0);
    await expect(main.getByRole('checkbox')).toHaveCount(0);
    await expect(main.getByRole('switch')).toHaveCount(0);
    const filters = page.getByRole('button', { name: 'Filters', exact: true });
    await filters.click();
    const panel = page.getByRole('dialog', { name: 'Filters', exact: true });
    await expect(panel.getByRole('combobox', { name: 'Owner', exact: true })).toBeVisible();
    const bounds = await panel.boundingBox();
    if (bounds === null) throw new Error('requested Filters must be visible');
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
    await expect(filters).toBeFocused();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
});

test('a member focuses explicit work while an ordinary readable reference stays outside it', async ({ page }) => {
  await inventory('vera', [
    inventoryRow({ id: id(49, 20), kind: 'project', name: 'Focus work', participating: true, href: '/project/work' }),
    inventoryRow({ id: id(49, 21), kind: 'project', name: 'Focus reference', participating: false, href: '/project/reference' }),
  ]);
  await signIn(page, PEOPLE.vera);
  await page.goto('/everything?q=Focus');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await expect(tree.getByRole('link', { name: 'Focus work', exact: true })).toBeVisible();
  await expect(tree.getByRole('link', { name: 'Focus reference', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Filters', exact: true }).click();
  await page.getByRole('switch', { name: 'Everything I can see', exact: true }).check();
  await page.keyboard.press('Escape');
  await expect(tree.getByRole('link', { name: 'Focus work', exact: true })).toBeVisible();
  await expect(tree.getByRole('link', { name: 'Focus reference', exact: true })).toHaveCount(0);
});

test('Active from filters activity independently of when the item was created', async ({ page }) => {
  await inventory('vera', [
    inventoryRow({ id: id(52, 1), kind: 'project', name: 'Activity current', created_at: '2026-10-01T09:00:00.000Z', last_activity: '2026-10-15T09:00:00.000Z', href: '/project/current' }),
    inventoryRow({ id: id(52, 2), kind: 'project', name: 'Activity earlier', created_at: '2026-10-01T09:00:00.000Z', last_activity: '2026-10-01T09:00:00.000Z', href: '/project/earlier' }),
  ]);
  await signIn(page, PEOPLE.vera);
  await page.goto('/everything?q=Activity');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await expect(tree.getByRole('link', { name: 'Activity earlier', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Filters', exact: true }).click();
  await page.getByLabel('Active from', { exact: true }).fill('2026-10-10');
  await expect(tree.getByRole('link', { name: 'Activity current', exact: true })).toBeVisible();
  await expect(tree.getByRole('link', { name: 'Activity earlier', exact: true })).toHaveCount(0);
  await page.getByLabel('Created from', { exact: true }).fill('2026-10-10');
  await expect(tree.getByRole('link', { name: 'Activity current', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await expect(tree.getByRole('link', { name: 'Activity earlier', exact: true })).toBeVisible();
  await expect(page.getByLabel('Active from', { exact: true })).toHaveValue('');
});

test('at 400 pixels filters fold away and complete rows use the page instead of a tiny scrolling table', async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 860 });
  await inventory('vera', [inventoryRow({ id: ACME.org, kind: 'org', name: 'Acme', href: '/org/acme', has_children: true }),
    ...Array.from({ length: 10 }, (_, number) => inventoryRow({ id: id(51, number + 1), kind: 'project',
      name: `A long project name that stays readable on a narrow screen ${number + 1}`, parent_id: ACME.org, href: `/project/${id(51, number + 1)}` })),
  ]);
  await signIn(page, PEOPLE.vera);
  await page.goto('/everything');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  const filters = page.getByRole('button', { name: 'Filters', exact: true });
  await expect(filters).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByRole('group', { name: 'Filters', exact: true })).toHaveCount(0);
  await expect(tree).toHaveCSS('display', 'block');
  await expect(tree.locator('..')).toHaveCSS('overflow-y', 'visible');
  await page.getByRole('button', { name: 'Expand all', exact: true }).click();
  const card = tree.locator('[data-everything-row]').filter({ hasText: 'A long project name that stays readable on a narrow screen 1' }).first();
  await expect(card).toBeVisible();
  await expect(card).toHaveCSS('display', 'grid');
  await expect(card.getByText('Owner', { exact: true })).toBeVisible();
  await expect(card.getByText('Last activity', { exact: true })).toBeVisible();
  await expect.poll(() => tree.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await filters.click();
  await expect(page.getByRole('dialog', { name: 'Filters', exact: true })).toBeVisible();
  await expect(page.getByLabel('Created from', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Sort', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Created', exact: true }).click();
  await expect(page.getByRole('group', { name: 'Sort', exact: true })).toContainText('Created');
  await page.keyboard.press('Escape');
  await expect(filters).toHaveAttribute('aria-expanded', 'false');
  await page.getByRole('button', { name: 'Columns', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Created', exact: true }).click();
  await expect(card.getByText('Created', { exact: true })).toBeVisible();
});

test('the admin column follows the session switch while Created remains the person’s choice', async ({ page }) => {
  await signIn(page);
  await page.goto('/everything');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await expect(tree.getByRole('columnheader', { name: 'As admin', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Columns', exact: true }).click();
  await expect(page.getByRole('menuitemcheckbox', { name: 'As admin', exact: true })).toHaveCount(0);
  await page.getByRole('menuitemcheckbox', { name: 'Created', exact: true }).click();
  await expect(tree.getByRole('columnheader', { name: 'Created', exact: true })).toBeVisible();
  await asAdmin(page, true);
  await expect(tree.getByRole('columnheader', { name: 'As admin', exact: true })).toBeVisible();
  await asAdmin(page, false);
  await expect(tree.getByRole('columnheader', { name: 'As admin', exact: true })).toHaveCount(0);
  await expect(tree.getByRole('columnheader', { name: 'Created', exact: true })).toBeVisible();
});

test('a member finds Everything in the rail, opens levels, chooses columns, sorts and searches names at every level', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('link', { name: 'Everything', exact: true }).click();
  await expect(page).toHaveURL(/\/everything/);
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await expect(tree).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Scope', exact: true })).toHaveCount(0);
  await tree.getByRole('button', { name: /^Expand Acme(?: · acme)?$/ }).click();
  await expect(tree.getByRole('link', { name: 'Infra · Acme', exact: true })).toBeVisible();
  await tree.getByRole('button', { name: 'Expand Research · Acme', exact: true }).click();
  await expect(tree.getByRole('link', { name: 'Web · Acme / Research', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Columns', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Created by', exact: true }).click();
  await expect(tree.getByRole('columnheader', { name: 'Created by', exact: true })).toBeVisible();
  await tree.getByRole('button', { name: 'Name', exact: true }).click();
  await expect(tree.getByRole('columnheader', { name: 'Name', exact: true })).toHaveAttribute('aria-sort', 'descending');
  await expect(page.getByRole('group', { name: 'Sort', exact: true })).toContainText('Sort: Name, descending');
  await expect(page.getByRole('button', { name: 'Filters', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await page.getByRole('searchbox', { name: 'Search by name', exact: true }).fill('fix login');
  await expect(tree.getByRole('link', { name: /Fix login/ }).first()).toBeVisible();
  await expect(tree.getByRole('link', { name: 'Infra · Acme', exact: true })).toHaveCount(0);
  await page.reload();
  await expect(tree.getByRole('columnheader', { name: 'Created by', exact: true })).toBeVisible();
  await expectAccessible(page, 'light', '[role="treegrid"]');
});

test('a shared leaf is reachable through minimal redacted path steps and a member is refused whole-system scope', async ({ page }) => {
  const outside = id(49, 1), hidden = id(49, 2), shared = id(49, 3);
  const path = [{ id: outside, kind: 'org' as const, name: null, redacted: true }, { id: hidden, kind: 'project' as const, name: null, redacted: true }];
  await inventory('vera', [
    inventoryRow({ id: outside, kind: 'org', name: null, has_children: true, created_at: null, my_access: 'path_only', path: path.slice(0, 1) }),
    inventoryRow({ id: hidden, kind: 'project', name: null, parent_id: outside, has_children: true, created_at: null, my_access: 'path_only', path }),
    inventoryRow({ id: shared, kind: 'topic', name: 'Shared release plan', parent_id: hidden, href: `/topic/${shared}`, path }),
  ]);
  await signIn(page, PEOPLE.vera);
  await page.goto('/everything');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await tree.getByRole('button', { name: 'Expand Shared with me', exact: true }).click();
  const root = tree.getByRole('row').filter({ hasText: 'Private place' });
  await expect(root).toContainText('Path only');
  await expect(root.getByRole('link')).toHaveCount(0);
  await expect(root.getByRole('button', { name: /Actions for/ })).toHaveCount(0);
  await root.getByRole('button', { name: 'Expand Private place', exact: true }).click();
  await tree.getByRole('button', { name: 'Expand Private place', exact: true }).click();
  await expect(tree.getByRole('link', { name: 'Shared release plan', exact: true })).toBeVisible();
  await page.goto('/everything?scope=system');
  await expect(page.getByText('You do not have access to this view. Choose what you can see instead.')).toBeVisible();
});

test('Management opens the same page with system scope and repeated department names carry their orgs', async ({ page }) => {
  const acme = id(50, 1), beta = id(50, 2), first = id(50, 3), second = id(50, 4);
  await inventory('ada', [
    inventoryRow({ id: acme, kind: 'org', name: 'Acme', has_children: true, href: `/org/${acme}` }),
    inventoryRow({ id: beta, kind: 'org', name: 'Beta', has_children: true, href: `/org/${beta}` }),
    inventoryRow({ id: first, kind: 'org', name: 'dep2', parent_id: acme, place_label: 'Acme', href: `/org/${first}` }),
    inventoryRow({ id: second, kind: 'org', name: 'dep2', parent_id: beta, place_label: 'Beta', href: `/org/${second}` }),
  ]);
  await signInForManagement(page);
  await page.goto('/admin/everything');
  await page.getByRole('link', { name: 'Open the whole system in Everything' }).click();
  await expect(page).toHaveURL(/\/everything\?scope=system/);
  await page.getByRole('button', { name: 'Filters', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Scope', exact: true })).toHaveText('System');
  await page.keyboard.press('Escape');
  const tree = page.getByRole('treegrid', { name: 'Everything I can see' });
  await page.getByRole('button', { name: 'Expand all', exact: true }).click();
  await expect(tree.getByRole('link', { name: 'dep2 · Acme', exact: true })).toBeVisible();
  await expect(tree.getByRole('link', { name: 'dep2 · Beta', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Collapse all', exact: true }).click();
  await expect(tree.getByRole('link', { name: /^dep2/ })).toHaveCount(0);
});
