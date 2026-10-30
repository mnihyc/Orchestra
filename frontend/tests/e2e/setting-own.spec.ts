/** A person edits only a place's own instructions, sees every ancestor and cannot bypass a higher lock. */
import { expect, test } from '@playwright/test';

import { ACME, deny, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41830'}`;
async function set(node: string, value: string, locked = false) {
  const response = await fetch(`${DOUBLE}/__double/day100/setting`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ node, key: 'agent.instructions', value, locked }) });
  expect(response.ok).toBe(true);
}

test.beforeEach(async () => {
  await scenario();
  await set(ACME.org, 'Team instruction');
  await set(ACME.research, 'Department instruction');
});

test('a blank own field never sends ancestors on first or repeated saves, and Reset removes only own', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=policies`);
  const row = page.locator('[data-setting="agent.instructions"]');
  const field = row.getByRole('textbox', { name: 'Instructions' });
  await expect(field).toHaveValue('');
  await expect(row.getByLabel('Inherited parts of Instructions')).toContainText('Team instruction');
  await expect(row.getByLabel('Inherited parts of Instructions')).toContainText('Department instruction');
  await field.fill('Project instruction');
  await row.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByRole('button', { name: 'Reset Instructions to inherited' })).toBeVisible();
  await field.fill('Project instruction twice');
  await row.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(field).toHaveValue('Project instruction twice');
  await row.locator('summary').click();
  await expect(row.locator('[data-resolved-preview] pre')).toHaveText('Team instruction\n\nDepartment instruction\n\nProject instruction twice');
  await row.getByRole('button', { name: 'Reset Instructions to inherited' }).click();
  await expect(field).toHaveValue('');
  await row.locator('summary').click();
  await expect(row.locator('[data-resolved-preview] pre')).toHaveText('Team instruction\n\nDepartment instruction');
});

test('an own value ignored by a higher lock is struck and the locking place and holders are named', async ({ page }) => {
  await set(ACME.web, 'Old local instruction');
  await set(ACME.org, 'Locked team instruction', true);
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=policies`);
  const row = page.locator('[data-setting="agent.instructions"]');
  await expect(row.locator('[data-own-part] pre')).toHaveText('Old local instruction');
  await expect(row.locator('[data-own-part] pre')).toHaveClass(/line-through/);
  await expect(row.getByRole('link', { name: 'Acme' }).last()).toHaveAttribute('href', '/org/acme');
  await expect(row).toContainText('Can change it:');
  await expect(row).toContainText('Ada Lovelace');
  await expect(row.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  await expect(row.getByRole('button', { name: 'Reset Instructions to inherited' })).toHaveCount(0);
});

test('late loss of permission preserves the failed own draft, and a reader never gets an enabled Save', async ({ page, browser }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=policies`);
  const row = page.locator('[data-setting="agent.instructions"]');
  await row.getByRole('textbox', { name: 'Instructions' }).fill('Keep this proposed instruction');
  await deny('ada', ['project.manage']);
  await row.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByRole('alert')).toBeVisible();
  await expect(row.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Keep this proposed instruction');
  const reader = await browser.newContext();
  try {
    const other = await reader.newPage();
    await signIn(other, PEOPLE.vera);
    await other.goto(`/project/${ACME.web}/settings?section=policies`);
    const otherRow = other.locator('[data-setting="agent.instructions"]');
    await expect(otherRow.getByRole('textbox', { name: 'Instructions' })).toBeDisabled();
    await expect(otherRow.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  } finally {
    await reader.close();
  }
});
