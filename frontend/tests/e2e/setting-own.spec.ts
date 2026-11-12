/** A person edits only a place's own instructions, sees every ancestor and cannot bypass a higher lock. */
import { expect, test, type Page } from '@playwright/test';

import { ACME, deny, DOUBLE, PEOPLE, scenario, signIn } from './support.ts';

async function set(node: string, value: string, locked = false) {
  const response = await fetch(`${DOUBLE}/__double/day100/setting`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ node, key: 'agent.instructions', value, locked }) });
  expect(response.ok).toBe(true);
}

test.beforeEach(async () => {
  await scenario();
  await set(ACME.org, 'Team instruction');
  await set(ACME.research, 'Department instruction');
});

const instructionRow = (page: Page) => page.locator('[data-setting="agent.instructions"]');
const editInstructions = (page: Page) => instructionRow(page).getByRole('button', { name: 'Edit Instructions', exact: true });
const instructionPath = `/api/v1/nodes/${ACME.web}/settings/agent.instructions`;

async function openInstructions(page: Page) {
  await editInstructions(page).click();
  const field = instructionRow(page).getByRole('textbox', { name: 'Instructions', exact: true });
  await expect(field).toBeVisible();
  return field;
}

test('a blank own field never sends ancestors on first or repeated saves, and Reset removes only own', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=defaults`);
  const row = instructionRow(page);
  const values: unknown[] = [];
  // Materialize the streamed request while preserving the native write and revision handling.
  await page.route((url) => url.pathname === instructionPath, async (route) => {
    if (route.request().method() === 'PUT') values.push((route.request().postDataJSON() as { value?: unknown } | null)?.value);
    await route.continue();
  });
  await expect(editInstructions(page)).toHaveAccessibleDescription(/Team instruction\s+Department instruction/);
  let field = await openInstructions(page);
  await expect(field).toHaveValue('');
  // Compact rows open their own editor on demand and commit multiline input with Ctrl+Enter.
  await field.fill('Project instruction');
  let accepted = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === instructionPath);
  await field.press('Control+Enter'); expect((await accepted).ok()).toBe(true);
  await expect(editInstructions(page)).toHaveAttribute('aria-expanded', 'false');
  field = await openInstructions(page);
  await expect(field).toHaveValue('Project instruction');
  await field.fill('Project instruction twice');
  accepted = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === instructionPath);
  await field.press('Control+Enter'); expect((await accepted).ok()).toBe(true);
  await expect(editInstructions(page)).toHaveAttribute('aria-expanded', 'false');
  expect(values).toEqual(['Project instruction', 'Project instruction twice']);
  await expect(editInstructions(page)).toHaveAccessibleDescription(/Team instruction\s+Department instruction\s+Project instruction twice/);
  await row.getByRole('button', { name: 'More about Instructions', exact: true }).click();
  const reset = page.waitForResponse((response) => response.request().method() === 'DELETE' && new URL(response.url()).pathname === instructionPath);
  await page.getByRole('menuitem', { name: 'Reset to inherited', exact: true }).click(); expect((await reset).ok()).toBe(true);
  await expect(editInstructions(page)).toHaveAccessibleDescription(/Team instruction\s+Department instruction/);
  await expect(editInstructions(page)).not.toHaveAccessibleDescription(/Project instruction/);
  field = await openInstructions(page);
  await expect(field).toHaveValue('');
});

test('a higher lock shows the effective value and source while retaining the ignored own value', async ({ page }) => {
  await set(ACME.web, 'Old local instruction');
  await set(ACME.org, 'Locked team instruction', true);
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=defaults`);
  const row = instructionRow(page);
  // The compact readout deliberately omits ignored drafts and links directly to the locking setting.
  await expect(row).toContainText('Locked team instruction');
  await expect(row).not.toContainText('Old local instruction');
  await expect(row.getByRole('link', { name: /Acme/ }).first()).toHaveAttribute('href', /\/org\/acme\/settings\/defaults\?setting=agent\.instructions/);
  await expect(editInstructions(page)).toHaveCount(0);
  await expect(row.getByRole('textbox', { name: 'Instructions', exact: true })).toHaveCount(0);
  await row.getByRole('button', { name: 'More about Instructions', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Reset to inherited', exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  // Relaxing the parent proves that hiding the ignored draft did not discard the stored own part.
  await set(ACME.org, 'Locked team instruction', false);
  await page.reload();
  const field = await openInstructions(page);
  await expect(field).toHaveValue('Old local instruction');
});

test('late loss of permission preserves the failed own draft, and a reader never gets an enabled Save', async ({ page, browser }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=defaults`);
  const row = instructionRow(page);
  const field = await openInstructions(page);
  await field.fill('Keep this proposed instruction');
  await deny('ada', ['project.manage', 'settings.manage']);
  const refused = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === instructionPath);
  await field.press('Control+Enter'); expect((await refused).status()).toBe(403);
  await expect(row.getByRole('alert')).toBeVisible();
  await expect(row.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Keep this proposed instruction');
  const reader = await browser.newContext();
  try {
    const other = await reader.newPage();
    await signIn(other, PEOPLE.vera);
    await other.goto(`/project/${ACME.web}/settings?section=defaults`);
    const otherRow = other.locator('[data-setting="agent.instructions"]');
    await expect(otherRow).toContainText('Team instruction');
    await expect(otherRow.getByRole('button', { name: 'Edit Instructions', exact: true })).toHaveCount(0);
    await expect(otherRow.getByRole('textbox', { name: 'Instructions', exact: true })).toHaveCount(0);
    await otherRow.getByRole('button', { name: 'More about Instructions', exact: true }).click();
    await expect(other.getByRole('menuitem', { name: 'Reset to inherited', exact: true })).toHaveCount(0);
  } finally {
    await reader.close();
  }
});
