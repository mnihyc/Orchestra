/** Every screen that is built, asked in both themes whether anything on it is a serious barrier, and worked by keyboard alone. */
import { expect, test } from '@playwright/test';

import { ACME, expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

// The gallery of the shared controls is part of a development build only: these two tests run against the development server.
test('the shared controls, every one of them drawn in both themes at once', { tag: '@development' }, async ({ page }) => {
  await page.goto('/dev/ui');
  await expect(page.getByRole('heading', { name: 'Shared controls' })).toBeVisible();

  await expectAccessibleInBothThemes(page);

  // What opens above the page is asked while it is open.
  await page.getByRole('button', { name: 'Topic menu' }).click();
  await expect(page.getByRole('menu', { name: 'Topic menu' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Archive Fix login' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Archive Fix login?', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  // The keyboard begins on the way out, never on the act.
  await expect(page.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Rename topic' }).click();
  await expect(page.getByRole('dialog', { name: 'Rename topic' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');
  // The keyboard returns to what opened the dialog.
  await expect(page.getByRole('button', { name: 'Rename topic' })).toBeFocused();

  await page.getByRole('button', { name: 'The export could not be finished' }).click();
  // The notice itself, not the line that reads it to a screen reader for a moment.
  const notices = page.getByRole('region', { name: /^Notifications/ });
  await expect(notices.getByText('The environment went offline while it was packed.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the shared controls in Chinese and at the compact density', { tag: '@development' }, async ({ page }) => {
  await page.goto('/dev/ui');
  await page.getByRole('radio', { name: '简体中文' }).click();
  await page.getByRole('radio', { name: '紧凑' }).click();

  await expect(page.getByRole('heading', { name: '共享控件' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-density', 'compact');
  await expectAccessibleInBothThemes(page);
});

test('the workbench round an org, a thread and a page that is still to come', async ({ page }) => {
  await signIn(page);
  await page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('link', { name: 'Acme' }).click();
  await expect(page.getByRole('treeitem', { name: /Fix login/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // With a tab's menu open, and with the list of shortcuts.
  await page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'Outline', exact: true }).click({ button: 'right' });
  await expect(page.getByRole('menu', { name: 'Actions for Outline' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');
  await page.locator('body').press('?');
  await expect(page.getByRole('dialog', { name: 'Keyboard shortcuts' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // Arranged for the code, with every area open.
  await page.getByRole('button', { name: 'Layout' }).click();
  await page.getByRole('menuitem', { name: 'Code' }).click();
  await expect(page.getByRole('tablist', { name: 'Bottom' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto('/usage');
  await expect(page.getByText('Usage is not built yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto('/nowhere');
  await expect(page.getByText('There is nothing at this address')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the workbench on a small screen, with its drawer open', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Navigation' }).click();
  await expect(page.getByRole('dialog', { name: 'Navigation' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the workbench is worked by keyboard alone: from area to area, into the tree, to a thread, and a view moved', async ({ page }) => {
  await signIn(page);
  await page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('link', { name: 'Acme' }).click();
  await expect(page.getByRole('treeitem', { name: /Fix login/ })).toBeVisible();

  // F6 goes from area to area; each stop puts the keyboard on something that takes it.
  const stops: string[] = [];
  await page.locator('body').click({ position: { x: 700, y: 500 } });
  for (let press = 0; press < 5; press += 1) {
    await page.keyboard.press('F6');
    stops.push(await page.evaluate(() => document.activeElement?.closest('[data-area-stop], [data-view-group]')?.getAttribute('aria-label') ?? ''));
  }
  expect(new Set(stops)).toEqual(new Set(['Orgs and your own places', 'Left bar', 'Main area', 'Right bar']));

  // Into the tree, down to a thread, and to its screen with Enter.
  await page.getByRole('treeitem', { name: /Research/ }).focus();
  await page.keyboard.type('fix');
  await expect(page.getByRole('treeitem', { name: /Fix login/ })).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('treeitem', { name: 'Main thread' }).first()).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));

  // A section moves by its keyboard menu; main and bottom groups retain numbered tab shortcuts.
  await page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'Outline', exact: true }).focus();
  await page.keyboard.press('Shift+F10');
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).press('ArrowRight');
  await page.getByRole('menuitem', { name: 'Bottom', exact: true }).press('Enter');
  await page.keyboard.press('Alt+2');
  await expect(page.getByRole('tab', { name: 'Outline', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Alt+w');
  await expect(page.getByRole('tab', { name: 'Outline', exact: true })).toHaveCount(0);
});
