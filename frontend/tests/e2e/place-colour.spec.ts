/** A place's colour is chosen by a click on its large icon on its page, by whoever may change it: one of the twelve, a
 * colour of its own from the small picker, or its kind's again; it is shown wherever the place's icon is. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const webIcon = (page: Page) => tree(page).getByRole('treeitem', { name: /^Web\b/ }).locator('[data-node-icon]').first();

test('the large icon opens the colour, and a colour of its own is drawn in the tree and on the page', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('button', { name: 'Change the colour of Web' }).click();
  const dialog = page.getByRole('dialog', { name: /^Colour of the project Web, in / });
  await expect(dialog).toBeVisible();

  // One of the twelve, at once.
  await dialog.getByRole('button', { name: 'Green' }).click();
  await expect(dialog.getByRole('button', { name: 'Green' })).toHaveAttribute('aria-pressed', 'true');
  await expect(webIcon(page)).toHaveAttribute('data-node-color', '5');

  // A colour of its own: picked, seen beside the picker, and given.
  await dialog.getByLabel('A colour of its own').fill('#3a7bd5');
  await dialog.getByRole('button', { name: 'Use this colour' }).click();
  await expect(dialog.getByRole('button', { name: 'Use this colour' })).toHaveAttribute('aria-pressed', 'true');
  await expect(dialog.getByRole('button', { name: 'Green' })).toHaveAttribute('aria-pressed', 'false');
  await expect(webIcon(page)).toHaveAttribute('data-node-color', 'own');
  await expect(webIcon(page)).toHaveAttribute('style', /--node-own: ?#3a7bd5/);

  // Its kind's again takes the colour of its own away.
  await dialog.getByRole('button', { name: 'As its kind' }).click();
  await expect(webIcon(page)).toHaveAttribute('data-node-color', '8');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Change the colour of Web' }).locator('[data-node-icon]')).toHaveAttribute('data-node-color', '8');
});

test('somebody who may not change the place sees its icon and no way to its colour', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('heading', { name: 'Web', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Change the colour of Web' })).toHaveCount(0);
});
