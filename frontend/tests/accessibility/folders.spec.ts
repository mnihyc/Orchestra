/** Every screen that holds or names folders, asked in both themes whether anything on it is a serious barrier: a project with its folder cards, a folder, the dialogs that make and move one with what stops it, a topic that names the folders it lies in, and the tree with a row closed over what waits. */
import { randomUUID } from 'node:crypto';

import { expect, type Page, test } from '@playwright/test';

import { ACME, changeTree, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });

test('a project that holds folders as a list, as a board and as Activity, and a folder of its own', async ({ page }) => {
  // A project with folders and no topic of its own: the cards stand above what says that it has no topics.
  await open(page, `/project/${ACME.shop}`);
  const folders = page.getByTestId('main').getByRole('region', { name: 'Folders' });
  await expect(folders.getByRole('link', { name: 'Checkout' })).toBeVisible();
  await expect(page.getByText('No topics yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // A folder that holds a folder and a topic: the same screen, as a list, as a board, and as Activity.
  await page.goto(`/project/${ACME.checkout}`);
  await expect(page.getByRole('heading', { name: 'Checkout', level: 1 })).toBeVisible();
  await expect(page.getByRole('table', { name: 'Topics' })).toBeVisible();
  await expect(folders.getByRole('link', { name: 'Payments' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('radio', { name: 'Board' }).click();
  await expect(folders.getByRole('link', { name: 'Payments' })).toBeVisible();
  await expect(page.getByRole('region', { name: /Idle/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('radio', { name: 'Activity' }).click();
  await expect(page.getByText('Checkout and the 1 folder in it')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // A search that leaves no folder, and a folder with nothing in it.
  await page.getByRole('radio', { name: 'List' }).click();
  await page.getByRole('searchbox', { name: 'Search topics' }).fill('zzz');
  await expect(page.getByText('No folder matches the filters.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto(`/project/${ACME.payments}`);
  await expect(page.getByRole('heading', { name: 'Payments', level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the dialogs of folders, open: a new folder, the list of where to move, a plan with people and a lock, and a move that is stopped', async ({ page }) => {
  await open(page, `/project/${ACME.checkout}`);

  await page.getByRole('button', { name: 'Actions for Checkout', exact: true }).last().click();
  await page.getByRole('menuitem', { name: 'New folder…', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'New folder in Checkout' })).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Create folder' }).click();
  // Said under the field, and by the field.
  await expect(page.getByRole('dialog').getByText('Give it a name.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.goto(`/topic/${ACME.cartRewrite}`);
  await page.getByRole('button', { name: 'Actions for Cart rewrite' }).last().click();
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const move = page.getByRole('dialog', { name: /^Move the topic Cart rewrite, in / });
  // The places of the tree are a tree of choices in the dialog itself.
  await expect(move.getByRole('radio', { name: 'Payments' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await move.getByRole('radio', { name: 'Payments' }).check();
  await expect(move.getByText('Cart rewrite moves from Checkout to Payments.')).toBeVisible();
  await expect(move.getByText('a lock at Checkout decides it')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // A move that the plan says cannot be: the folders under Shop go eight deep, and Infra is one more.
  let above = ACME.payments;
  const chain = ['Level 4', 'Level 5', 'Level 6', 'Level 7', 'Level 8'].map((name) => {
    const made = { id: randomUUID(), kind: 'project', name, parent_id: above };
    above = made.id;
    return made;
  });
  await changeTree(chain);
  await page.goto(`/project/${ACME.infra}`);
  await page.getByRole('button', { name: 'Actions for Infra' }).last().click();
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const stopped = page.getByRole('dialog', { name: /^Move the project Infra, in / });
  await stopped.getByRole('radio', { name: 'Level 8' }).check();
  await expect(stopped.getByText('Folders cannot be nested any deeper here.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a topic that lies in folders, a folder’s menu, and the tree with a row closed over what waits', async ({ page }) => {
  await open(page, `/topic/${ACME.cardFees}`);
  await expect(page.getByRole('heading', { name: 'Card fees' })).toBeVisible();
  await expect(page.getByTestId('main').getByRole('link', { name: 'Checkout' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // On the screen of a place everything above it is open, so the row is closed from a screen that is not below it.
  await page.goto('/org/acme');
  await changeTree([{ id: ACME.feeTable, thread: { state: 'waiting_approval', blocked_reason: null, pending_approvals: 2, activity_seq: 2, last_activity_at: '2026-10-11T10:00:00.000Z' } }]);
  await expect(tree(page).getByRole('treeitem', { name: /^Checkout/ })).toBeVisible();
  await tree(page).getByRole('treeitem', { name: /^Checkout/ }).focus();
  await page.keyboard.press('ArrowLeft');
  await expect(tree(page).getByRole('treeitem', { name: /^Checkout/ })).toContainText('2 approvals wait inside');
  await expectAccessibleInBothThemes(page);

  await tree(page).getByRole('treeitem', { name: /^Checkout/ }).click({ button: 'right' });
  await expect(page.getByRole('menu', { name: 'Menu of Checkout' })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'New folder…' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('folders on a small screen, and in Chinese at the compact density', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await open(page, `/project/${ACME.checkout}`);
  await expect(page.getByRole('heading', { name: 'Checkout', level: 1 })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Folders' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto(`/topic/${ACME.cardFees}`);
  await expect(page.getByRole('heading', { name: 'Card fees' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The language and the density are the person's own, kept on the server and chosen on Interface.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/settings/interface');
  await page.getByRole('radio', { name: 'Compact' }).click();
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-density', 'compact');
  await page.goto(`/project/${ACME.checkout}`);
  await expect(page.getByTestId('main').getByRole('region', { name: '文件夹' })).toBeVisible();
  await expect(page.getByRole('button', { name: '新建文件夹' }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: '新建文件夹' }).first().click();
  await expect(page.getByRole('dialog', { name: '在 Checkout 中新建文件夹' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
