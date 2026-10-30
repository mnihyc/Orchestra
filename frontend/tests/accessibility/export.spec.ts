/** Every screen of taking things away, asked in both themes whether anything on it is a serious barrier: the dialog of a thread, the dialog of a place in its several states (all ticked, past a limit, greyed rows, all options visible), the form that asks for the right to export, the list of one's exports, and the notices of a job. */
import { expect, type Page, test } from '@playwright/test';

import { id } from '../doubles/fixtures.ts';
import { ACME, changeTree, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function steer(path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/export${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
}

test.beforeEach(async () => {
  await scenario();
});

const dialog = (page: Page) => page.getByRole('dialog', { name: /^(Export|Request access to export) / });

async function openFor(page: Page, place: string): Promise<void> {
  await page.getByRole('button', { name: `Actions for ${place}` }).first().click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  await expect(dialog(page).getByRole('treeitem').first()).toBeVisible();
  await expect(dialog(page).locator('#export-plan')).toContainText(/allowed here|Nothing|allows no/);
}

test('the dialog of a thread, with its choices and its refusal of a turn that is none', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for Main thread/ }).click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  await expect(dialog(page)).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('radio', { name: 'JSON', exact: true }).click();
  await dialog(page).getByLabel('From turn').fill('0');
  await dialog(page).getByRole('button', { name: 'Export', exact: true }).click();
  await expect(dialog(page).getByText(/is not a turn/)).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the dialog of a place with everything ticked, a place half ticked, and all options visible', async ({ page }) => {
  await changeTree([
    { id: id(3, 101), kind: 'project', name: 'Returns', parent_id: ACME.shop },
    { id: id(3, 102), kind: 'project', name: 'Search', parent_id: ACME.shop },
  ]);
  await signIn(page);
  await page.goto(`/project/${ACME.shop}`);
  await openFor(page, 'Shop');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('treeitem', { name: /^Search/ }).click();
  await expect(dialog(page).getByRole('treeitem', { name: /^Shop/ })).toHaveAttribute('aria-checked', 'mixed');
  await expect(dialog(page).getByRole('combobox', { name: /Conversations/ })).toBeVisible();
  await expect(dialog(page).getByRole('checkbox', { name: 'Attachments' })).toBeVisible();
  await expect(dialog(page).getByLabel('From', { exact: true })).toBeVisible();
  await expect(dialog(page).getByLabel('To', { exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the dialog of a place past what it allows, and one that allows nothing', async ({ page }) => {
  await steer('/limits', { max_bytes: 30 * 1024 });
  await signIn(page);
  await page.goto(`/project/${ACME.shop}`);
  await openFor(page, 'Shop');
  await expect(dialog(page).locator('#export-plan')).toContainText('Choose less.');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await steer('/limits', { max_bytes: 0 });
  await openFor(page, 'Shop');
  await expect(dialog(page).locator('#export-plan')).toContainText('allows no export');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the dialog of a place with a row greyed and its things counted', async ({ page }) => {
  await steer('/bar', { username: 'dora', permission: 'thread.export', object: ACME.repro });
  await signIn(page, PEOPLE.dora);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await openFor(page, 'Fix login');
  await expect(dialog(page).getByRole('treeitem', { name: /^Repro/ })).toHaveAttribute('aria-checked', 'false');
  await expect(dialog(page).locator('#export-plan')).toContainText('left out');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the menu of a thread that may be read and not exported, and the form that asks for the right', async ({ page }) => {
  await steer('/bar', { username: 'dora', permission: 'thread.export', object: ACME.repro });
  await signIn(page, PEOPLE.dora);
  await page.goto(`/thread/${ACME.repro}`);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Actions for Repro', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: /^Export…/ })).toHaveAttribute('aria-disabled', 'true');
  await expectAccessibleInBothThemes(page, '[role="menu"]');

  await page.getByRole('menuitem', { name: 'Request access' }).click();
  await expect(dialog(page)).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the list of one’s exports with an archive of every state', async ({ page }) => {
  await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
  await steer('/seed-archive', { state: 'pending', place: ACME.infra, ago_ms: 5 * 60_000 });
  await steer('/seed-archive', { state: 'failed', place: ACME.shop, ago_ms: 3 * 3_600_000 });
  await steer('/seed-archive', { state: 'expired', place: ACME.research, ago_ms: 5 * 3_600_000, expires_in_ms: 10 * 3_600_000 });
  await steer('/seed-archive', { state: 'expired', place: ACME.payroll, ago_ms: 30 * 3_600_000, expires_in_ms: -3_600_000 });
  await signIn(page);
  await page.goto('/settings/exports');
  await expect(page.getByRole('table', { name: 'Your exports' }).getByRole('row')).toHaveCount(6);
  await expectAccessibleInBothThemes(page);

  await page.getByRole('row', { name: /Web/ }).getByRole('button', { name: /Actions for the archive of Web/ }).click();
  await page.getByRole('menuitem', { name: 'Remove' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Remove the archive of Web?' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"]');
});

test('the list with nothing in it, and a narrow screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/settings/exports');
  await expect(page.getByText('Nothing exported yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
  await page.setViewportSize({ width: 400, height: 800 });
  await page.goto('/settings/exports');
  await expect(page.getByRole('row', { name: /Web/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the notice of a job that ended, and the part of the status bar that says so', async ({ page }) => {
  await steer('/jobs', { hold: true });
  await signIn(page);
  await page.goto(`/project/${ACME.shop}`);
  await openFor(page, 'Shop');
  await dialog(page).getByRole('button', { name: 'Export', exact: true }).click();
  await expect(page.getByRole('contentinfo', { name: 'Status' }).getByText(/Exporting Shop/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await steer('/finish');
  await expect(page.getByRole('region', { name: /^Notifications/ }).getByRole('listitem').filter({ hasText: 'Shop: export ready' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
