/** Mark done and Reopen each have one Undo, made on the revision the change returned. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const row = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^${name}( |$)`) });

async function rowMenu(page: Page, name: string): Promise<Locator> {
  await row(page, name).hover();
  await row(page, name).getByRole('button', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true }).click();
  return page.getByRole('menu', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true });
}

test('a topic marked done is open again with the Undo of its notice, and reopened is done again', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Fix login')).toBeVisible();

  await (await rowMenu(page, 'Fix login')).getByRole('menuitem', { name: 'Mark done' }).click();
  await expect(row(page, 'Fix login')).toHaveAccessibleName(/^Fix login topic, done/);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(row(page, 'Fix login')).not.toHaveAccessibleName(/done/);

  await (await rowMenu(page, 'Migrate CI')).getByRole('menuitem', { name: 'Reopen' }).click();
  await expect(row(page, 'Migrate CI')).not.toHaveAccessibleName(/done/);
  // The newest notice is the one of this change.
  await page.getByRole('button', { name: 'Undo', exact: true }).last().click();
  await expect(row(page, 'Migrate CI')).toHaveAccessibleName(/^Migrate CI topic, done/);
});
