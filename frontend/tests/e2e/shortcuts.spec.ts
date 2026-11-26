/** Shortcuts at work: Add shortcut… on a place keeps one in another place chosen with the place picker; it stands under that place in the tree with its target's name and the arrow, leads to the target, moves by dragging onto another place, and its removal leaves the target as it was. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation', exact: true });
const tree = (page: Page) => page.getByRole('tree', { name: 'Organizations, projects, topics and threads' });
/** A row of the tree that is a place, by its name and the kind its glyph says (a shortcut's row says it is one instead). */
const row = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^${name} (org|project|folder|topic)( |$)`) });
/** The row of a shortcut, which says after its target's name that it is one. */
const shortcutRow = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^${name} shortcut to a `) });

async function rowMenu(page: Page, of: Locator, name: string): Promise<Locator> {
  await of.hover();
  await of.getByRole('button', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`) }).click();
  return page.getByRole('menu', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`) });
}

test('a shortcut is kept in a chosen place, leads to its target, moves by dragging and goes without touching the target', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Infra')).toBeVisible();

  // Add shortcut… on Infra, kept in Web.
  const menu = await rowMenu(page, row(page, 'Infra'), 'Infra');
  await menu.getByRole('menuitem', { name: 'Add shortcut…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add shortcut to “Infra”' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('combobox', { name: 'Place' }).click();
  await page.getByRole('searchbox', { name: 'Type part of a name…' }).fill('Web');
  await page.getByRole('tree', { name: 'Place' }).getByRole('treeitem', { name: /Web/ }).first().click();
  await dialog.getByRole('button', { name: 'Add shortcut' }).click();
  await expect(page.getByText('Shortcut to “Infra” added to Web')).toBeVisible();

  // Under Web (a project, open in the tree as shipped) it stands with Infra's name, the arrow, and the way to Infra.
  const kept = shortcutRow(page, 'Infra');
  await expect(kept).toBeVisible();
  await expect(kept.locator('[data-shortcut-icon="project"] > span')).toHaveCount(2);
  const link = kept.getByRole('link');
  await expect(link).toHaveAttribute('href', /^\/project\//);

  // Dragged onto Infra itself, it is moved there and leaves Web.
  await link.dragTo(row(page, 'Infra').getByRole('link').first());
  await expect(page.getByText('Shortcut “Infra” moved to Infra')).toBeVisible();
  await expect(bar(page).locator('[data-shortcut]').filter({ hasText: 'Infra' }).first()).toBeAttached();

  // Removed, it is gone; Infra stays where it was.
  const moved = shortcutRow(page, 'Infra');
  await expect(moved).toBeVisible();
  const own = await rowMenu(page, moved, 'Infra');
  await own.getByRole('menuitem', { name: 'Remove shortcut' }).click();
  await expect(page.getByText('Shortcut “Infra” removed')).toBeVisible();
  await expect(shortcutRow(page, 'Infra')).toHaveCount(0);
  await expect(row(page, 'Infra')).toBeVisible();
});
