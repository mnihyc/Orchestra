/** The resources screens as a table and a page are expected to be, asked in both themes whether anything on them is a serious barrier: the row of kinds above each list, the act at the end of a row and the menu it opens, rows that are ticked with the acts they share, the Edit dialog of each kind, a hint open beside an option, the dialog that enrolls a machine again, and the same on a phone. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

/** The row of a list that holds the link of a thing. */
const rowOf = (page: Page, table: string, name: string): Locator =>
  page.getByRole('table', { name: table }).getByRole('row').filter({ has: page.getByRole('link', { name, exact: true }) });

const LISTS = [
  { path: '/resources/providers', table: 'Providers', thing: 'OpenAI key' },
  { path: '/resources/agents', table: 'Agent profiles', thing: 'Claude Code' },
  { path: '/resources/machines', table: 'Machines', thing: 'ada-laptop' },
  { path: '/resources/environments', table: 'Environments', thing: 'Workspace' },
  { path: '/resources/templates', table: 'Templates', thing: 'Node 22 dev' },
  { path: '/resources/hosts', table: 'Hosts', thing: 'host-1' },
] as const;

test('every list has its row of kinds above it, the act of a row at its end and the menu of the rest', async ({ page }) => {
  await signIn(page);
  for (const list of LISTS) {
    await page.goto(list.path);
    await expect(page.getByRole('table', { name: list.table })).toBeVisible();
    await expect(page.getByRole('navigation', { name: 'Kinds of resources' })).toBeVisible();
    await expectAccessibleInBothThemes(page);

    // The act most used is on the row when the row is pointed at, and has its name when the keyboard reaches it.
    const row = rowOf(page, list.table, list.thing);
    await row.hover();
    await expect(row.getByRole('button', { name: new RegExp(` ${list.thing}$`) }).first()).toHaveCSS('opacity', '1');
    await expectAccessibleInBothThemes(page);

    await row.getByRole('button', { name: `Actions for ${list.thing}` }).click();
    await expect(page.getByRole('menu', { name: `Actions for ${list.thing}` })).toBeVisible();
    await expectAccessibleInBothThemes(page, '[role="menu"]');
    await page.keyboard.press('Escape');
  }
});

test('rows that are ticked show the acts they share, and the question that names them', async ({ page }) => {
  await signIn(page);
  await page.goto('/resources/agents');
  await page.getByRole('checkbox', { name: 'Select Codex', exact: true }).check();
  await page.getByRole('checkbox', { name: 'Select Scripted agent' }).check();
  const bar = page.getByRole('toolbar', { name: 'Acts on the ticked rows' });
  await expect(bar.getByText('2 selected')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await bar.getByRole('button', { name: 'Disable…' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Disable 2 profiles?' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the menu under the right mouse button, and the dialog that renames from it', async ({ page }) => {
  await signIn(page);
  await page.goto('/resources/environments');
  await rowOf(page, 'Environments', 'Workspace').click({ button: 'right' });
  await expect(page.getByRole('menu', { name: /^Actions for Workspace(?: · .+)?$/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.getByRole('menuitem', { name: 'Rename…' }).click();
  await expect(page.getByRole('dialog', { name: 'Rename Workspace' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the Edit dialog of each kind, with what cannot be changed read only beside its reason', async ({ page }) => {
  await signIn(page);
  const edit = async (path: string, title: string) => {
    await page.goto(path);
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await expect(page.getByRole('dialog', { name: title })).toBeVisible();
    await expectAccessibleInBothThemes(page);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog', { name: title })).toBeHidden();
  };
  await edit(`/resources/providers/${RUNS_ON.openAiKey}`, 'Edit OpenAI key');
  await edit(`/resources/agents/${RUNS_ON.codexOnLaptop}`, 'Edit Codex on ada-laptop');
  await edit(`/resources/machines/${RUNS_ON.laptop}`, 'Edit ada-laptop');
  await edit(`/resources/environments/${RUNS_ON.workspace}`, 'Edit Workspace');

  await page.goto('/resources/templates');
  await page.getByRole('link', { name: 'Node 22 dev', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Edit Node 22 dev' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.goto('/resources/hosts');
  await page.getByRole('link', { name: 'host-1', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Edit host-1' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a hint opens the same card beside an option on a desktop and a phone', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('button', { name: 'Help: Allow writing from outside' }).click();
  await expect(page.getByRole('dialog', { name: 'Allow writing from outside' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // Inside a dialog, where the hint stands over the dialog's own layer.
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Workspace' });
  await dialog.getByRole('button', { name: 'Help: Allow a terminal' }).click();
  await expect(page.getByRole('dialog', { name: 'Allow a terminal' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');

  await page.setViewportSize({ width: 390, height: 800 });
  await page.getByRole('button', { name: 'Help: Allow a terminal' }).click();
  await expect(page.getByRole('dialog', { name: 'Allow a terminal' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a machine that cannot connect any more is enrolled again, and its certificate is on its own tab', async ({ page }) => {
  await signIn(page);
  await page.goto('/resources/machines');
  const row = rowOf(page, 'Machines', 'build-box');
  await row.hover();
  await row.getByRole('button', { name: 'Enroll again… build-box' }).click();
  const dialog = page.getByRole('dialog', { name: 'Enroll build-box again' });
  await expect(dialog.getByText(/--token 'oenr_/).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await row.getByRole('link', { name: 'build-box', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'build-box', level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('tab', { name: 'Certificate' }).click();
  await expect(page.getByRole('tab', { name: 'Certificate', selected: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('on a phone the lists and the pages keep the way back, the acts and the menu', async ({ page }) => {
  await signIn(page);
  await page.setViewportSize({ width: 390, height: 800 });
  for (const list of [LISTS[1], LISTS[3], LISTS[5]]) {
    await page.goto(list.path);
    await expect(page.getByRole('table', { name: list.table })).toBeVisible();
    await expectAccessibleInBothThemes(page);
    await rowOf(page, list.table, list.thing).getByRole('button', { name: `Actions for ${list.thing}` }).click();
    await expect(page.getByRole('menu', { name: `Actions for ${list.thing}` })).toBeVisible();
    await expectAccessibleInBothThemes(page, '[role="menu"]');
    await page.keyboard.press('Escape');

    await page.getByRole('link', { name: list.thing, exact: true }).click();
    await expect(page.getByRole('heading', { name: list.thing, level: 1 })).toBeVisible();
    await expect(page.getByRole('link', { name: /^Back to / })).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
});

test('somebody who may only read is offered the acts they may take, and no box to tick a row', async ({ page }) => {
  await signIn(page, PEOPLE.dora);
  await page.goto('/resources/providers?view=usable');
  const row = rowOf(page, 'Providers', 'Team Anthropic');
  await expect(row).toBeVisible();
  await expect(page.getByRole('checkbox', { name: /^Select / })).toHaveCount(0);
  await expectAccessibleInBothThemes(page);
  await row.click({ button: 'right' });
  await expect(page.getByRole('menu', { name: /^Actions for Team Anthropic(?: · .+)?$/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
});
