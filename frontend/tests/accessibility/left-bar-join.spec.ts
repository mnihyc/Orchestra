/** What scrolls takes the keyboard, in both themes: the menu of a place opened by the pointer and the list of keys asked from the tree, in a window that is not tall, and the dialogs and menus that do not scroll do not become a stop for it. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });

test('the menu of a place that is longer than the room it has scrolls, takes the keyboard, and is no barrier to anybody', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 480 });
  await signIn(page);
  await page.goto('/org/acme');
  await expect(tree(page).getByRole('treeitem', { name: /^Web/ })).toBeVisible();

  // Opened by the pointer: the menu has no item in the order of the keys until the keyboard comes into it.
  await tree(page).getByRole('treeitem', { name: /^Web/ }).hover();
  await bar(page).getByRole('button', { name: /^Actions for Web(?: · .+)?$/, exact: true }).click();
  const menu = page.getByRole('menu', { name: /^Actions for Web(?: · .+)?$/, exact: true });
  await expect(menu).toBeVisible();
  expect(await menu.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
  await expect(menu).toHaveAttribute('tabindex', '0');
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.keyboard.press('Escape');

  // A short menu is no stop of its own.
  await page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('button', { name: 'New org or join one' }).click();
  const short = page.getByRole('menu', { name: 'New org or join one' });
  await expect(short).toBeVisible();
  expect(await short.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(false);
  await expect(short).not.toHaveAttribute('tabindex', '0');
});

test('the list of keys asked from the tree is longer than a short window, scrolls, takes the keyboard and is named for what it lists', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 520 });
  await signIn(page);
  await page.goto('/org/acme');
  await expect(tree(page).getByRole('treeitem', { name: /^Web/ })).toBeVisible();
  await tree(page).getByRole('treeitem', { name: /^Web/ }).focus();
  await page.keyboard.press('?');
  const dialog = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  await expect(dialog).toBeVisible();
  const body = dialog.getByRole('region', { name: 'Keyboard shortcuts' });
  await expect(body).toBeVisible();
  expect(await body.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
  await expect(body).toHaveAttribute('tabindex', '0');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  // The keyboard reads the rest of it: a key on the body scrolls it.
  await body.focus();
  await page.keyboard.press('End');
  expect(await body.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  // In a tall window the same list fits, and its body is no stop.
  await page.setViewportSize({ width: 1280, height: 1400 });
  await tree(page).getByRole('treeitem', { name: /^Web/ }).focus();
  await page.keyboard.press('?');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('region')).toHaveCount(0);
});
