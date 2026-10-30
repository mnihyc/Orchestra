/** A place's own colour follows its cards, remote finder names, export choices and shared list. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

test('a custom colour reaches cards, the finder and the export tree', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('button', { name: 'Change the colour of Web' }).click();
  const colour = page.getByRole('dialog', { name: /^Colour of the project Web/ });
  await colour.getByLabel('A colour of its own').fill('#3a7bd5');
  await colour.getByRole('button', { name: 'Use this colour', exact: true }).click();
  await page.keyboard.press('Escape');
  await page.goto('/org/acme');
  const card = page.getByRole('article').filter({ has: page.getByRole('link', { name: 'Web', exact: true }) });
  await expect(card.locator('[data-node-icon]').first()).toHaveAttribute('data-node-color', 'own');
  await page.getByRole('button', { name: 'Find anything', exact: true }).click();
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('Web');
  const hit = finder.getByRole('option', { name: /^Web\b/ }).first();
  await expect(hit.locator('[data-node-icon]')).toHaveAttribute('style', /--node-own: ?#3a7bd5/);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Actions for Web', exact: true }).first().click();
  await page.getByRole('menuitem', { name: 'Export…', exact: true }).click();
  const exportDialog = page.getByRole('dialog', { name: 'Export Web', exact: true });
  await expect(exportDialog.getByRole('treeitem', { name: /^Web\b/ }).locator('[data-node-icon]').first()).toHaveAttribute('data-node-color', 'own');
});

test('a shared place keeps its own colour even when its tree is not held', async ({ page }) => {
  await signIn(page);
  const response = await page.request.get(`/api/v1/nodes/${ACME.web}`);
  const node = await response.json() as Record<string, unknown>;
  await page.route('**/api/v1/shared', async (route) => { await route.fulfill({ json: { items: [{ ...node, custom_color: '#112233', color: null }], partial: false, next_cursor: null } }); });
  await page.goto('/shared');
  const row = page.getByRole('table', { name: 'Shared with me', exact: true }).getByRole('row').filter({ has: page.getByRole('link', { name: 'Web', exact: true }) });
  await expect(row.locator('[data-node-icon]')).toHaveAttribute('style', /--node-own: ?#112233/);
});
