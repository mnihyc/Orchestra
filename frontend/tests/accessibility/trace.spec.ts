/** Narrow keyboard flows and accessible trail, retained content and removal states in both themes. */
import { expect, test } from '@playwright/test';
import { ACME, RUNS_ON, TRACE } from '../doubles/fixtures';
import { expectAccessibleSettled, scenario, signIn } from '../e2e/support';

test.beforeEach(async () => { await scenario(); });
test('the phone trail keeps rows visible, opens filters by keyboard and has no serious barrier in either theme', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await signIn(page); await page.goto('/org/acme/settings/audit');
  const filters = page.locator('summary').filter({ hasText: 'Filters' }); await filters.focus(); await page.keyboard.press('Enter'); await page.locator('summary').filter({ hasText: /^More$/ }).click(); await expect(page.getByRole('combobox', { name: 'Exact action' })).toBeVisible();
  const group = page.getByRole('combobox', { name: 'Group by' }); await group.focus(); await page.keyboard.press('Enter'); await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter'); await expect(group).toContainText('Kind of act');
  await page.reload(); await expect(page.getByRole('combobox', { name: 'Group by' })).toContainText('Kind of act'); expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true); await expectAccessibleSettled(page);
});
test('retained conversation and document have read-only keyboard content and no serious barrier', async ({ page }) => {
  await signIn(page); await page.goto(`/inspect/thread/${TRACE.thread}`); await expect(page.locator('[data-stored-timeline]')).toBeVisible(); await expectAccessibleSettled(page);
  await page.goto(`/inspect/document/${TRACE.document}`); await expect(page.getByRole('textbox', { name: 'Document text' })).toHaveAttribute('contenteditable', 'false'); await expectAccessibleSettled(page);
});
test('kept rows and explicit purge and Remove confirmations fit a phone and remain accessible', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await signIn(page); await page.goto(`/project/${ACME.web}`); await page.getByRole('button', { name: /^Actions for Web(?: · .+)?$/, exact: true }).first().click(); await page.getByRole('menuitem', { name: 'Recently deleted…', exact: true }).click(); await page.locator(`[data-kept-environment="${TRACE.environment}"]`).getByRole('button', { name: 'Remove for good', exact: true }).click(); await expect(page.getByRole('dialog', { name: 'Remove Kept workspace for good' })).toBeVisible(); await expectAccessibleSettled(page, '[role="dialog"]:last-of-type'); await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
  await page.goto(`/resources/environments/${RUNS_ON.sandbox}`); await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click(); await page.getByRole('menuitem', { name: 'Remove…', exact: true }).click(); await expectAccessibleSettled(page, '[role="dialog"]');
});
