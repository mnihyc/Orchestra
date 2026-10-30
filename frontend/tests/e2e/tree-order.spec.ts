/** The order of the tree: each person's own choice in each org, kept by the server for every device, and the org's own order, which an owner or admin saves from the order they see. */
import { expect, type Page, test } from '@playwright/test';

import { scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });

async function viewMenu(page: Page) {
  await bar(page).getByRole('button', { name: 'View options' }).click();
  return page.getByRole('menu', { name: 'View options' });
}

/** The places that lie directly in Acme, in the order the tree shows them. */
async function top(page: Page): Promise<string[]> {
  const names = await tree(page).locator('[role="treeitem"] a > span.truncate').allTextContents();
  return names.filter((name) => ['Research', 'Infra', 'Payroll', 'Shop'].includes(name));
}

test('a person orders their tree for every device, and an admin makes that order the org’s own', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  // The fake world's team keeps its tree by name; a department stands before the projects in every order.
  await expect.poll(() => top(page)).toEqual(['Research', 'Infra', 'Payroll', 'Shop']);

  await (await viewMenu(page)).getByRole('menuitemradio', { name: 'Newest' }).click();
  await expect.poll(() => top(page)).toEqual(['Research', 'Shop', 'Payroll', 'Infra']);
  // The server keeps it: the page read again, as another device would, shows the same.
  await page.reload();
  await expect.poll(() => top(page)).toEqual(['Research', 'Shop', 'Payroll', 'Infra']);

  // The admin makes it the order of everybody who follows the org's own, and follows it themself.
  await (await viewMenu(page)).getByRole('menuitem', { name: 'Save for everyone in Acme' }).click();
  await expect(page.getByText('Everyone in Acme now sees this order unless they chose their own')).toBeVisible();
  await (await viewMenu(page)).getByRole('menuitemradio', { name: 'Defaults' }).click();
  await expect.poll(() => top(page)).toEqual(['Research', 'Shop', 'Payroll', 'Infra']);
  await (await viewMenu(page)).getByRole('menuitemradio', { name: 'Name' }).click();
  await expect.poll(() => top(page)).toEqual(['Research', 'Infra', 'Payroll', 'Shop']);
});

test('a row dropped on another place opens Move to… with that place chosen, and nothing moves until it is confirmed', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const row = (name: string) => tree(page).getByRole('treeitem', { name: new RegExp(`^${name}( |$)`) });
  await expect(row('Infra')).toBeVisible();
  // Web lies in Research: its topics show once both are open.
  await row('Research').press('ArrowRight');
  await expect(row('Web')).toBeVisible();
  await row('Web').press('ArrowRight');
  await expect(row('Upgrade deps')).toBeVisible();

  await row('Upgrade deps').getByRole('link').first().dragTo(row('Infra').getByRole('link').first());
  const dialog = page.getByRole('dialog', { name: /^Move the topic Upgrade deps, in / });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('radio', { name: 'Infra' })).toBeChecked();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toHaveCount(0);
  // Still where it was.
  await expect(row('Upgrade deps')).toBeVisible();
});

test('a row dropped beside another of its parent stands there, in the reader’s own order from the one they saw', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const row = (name: string) => tree(page).getByRole('treeitem', { name: new RegExp(`^${name}( |$)`) });
  await expect.poll(() => top(page)).toEqual(['Research', 'Infra', 'Payroll', 'Shop']);
  // Dropped near the top edge of Infra: Shop stands before it.
  await row('Shop').getByRole('link').first().dragTo(row('Infra').getByRole('link').first(), { targetPosition: { x: 12, y: 2 } });
  await expect.poll(() => top(page)).toEqual(['Research', 'Shop', 'Infra', 'Payroll']);
  await expect((await viewMenu(page)).getByRole('menuitemradio', { name: 'Custom' })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await page.reload();
  await expect.poll(() => top(page)).toEqual(['Research', 'Shop', 'Infra', 'Payroll']);
});
