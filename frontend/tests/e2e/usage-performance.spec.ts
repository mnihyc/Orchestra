/** The Performance view reads the same scoped fake-world routes as Usage and Requests. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test.beforeEach(async ({ request }) => { await scenario(); expect((await request.post(`${DOUBLE}/__double/usage/performance`)).ok()).toBe(true); });

test('shows measurements, changes chart measure, sorts models, and narrows the shared screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage?view=performance&range=7d');
  const figures = page.getByRole('list', { name: 'Performance figures' });
  await expect(figures).toContainText('tok/s');
  await expect(figures).toContainText('(1)');
  const table = page.getByRole('table', { name: 'Compare models', exact: true });
  await expect(table).toContainText('hidden-model');
  await expect(table.getByRole('row').filter({ hasText: 'hidden-model' })).toContainText('—');
  await page.getByRole('combobox', { name: 'Measure', exact: true }).click();
  await page.getByRole('option', { name: 'First output', exact: true }).click();
  await expect(page).toHaveURL(/show=first_token/);
  await expect(page.getByText('Line: median.', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Sort by Median speed', exact: true }).click();
  await expect(table.getByRole('row').nth(1)).toContainText('fast-model');
  await table.getByRole('button', { name: /fast-model/ }).click();
  await expect(page).toHaveURL(/model=fast-model/);
  await page.getByRole('tab', { name: 'Requests', exact: true }).click();
  await expect(page).toHaveURL(/model=fast-model/);
  const requests = page.getByRole('table', { name: 'Requests', exact: true });
  await expect(requests).toContainText('fast-model');
  await expect(requests).not.toContainText('slow-model');
});
