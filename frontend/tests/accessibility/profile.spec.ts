/** The person's page and its settings remain readable and operable in both themes. */
import { expect, test } from '@playwright/test';
import { expectAccessibleSettled, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => { await scenario(); });
test('the page, calendar table and Your page settings', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto('/u/ada');
  await expect(page.getByRole('button', { name: 'Show as a table' })).toBeVisible();
  await expectAccessibleSettled(page);
  await page.getByRole('button', { name: 'Show as a table' }).click();
  await expectAccessibleSettled(page);
  await page.goto('/settings/your-page');
  await expect(page.getByRole('heading', { name: 'Your page', exact: true, level: 1 })).toBeVisible();
  await expectAccessibleSettled(page);
});
