/** Requests pause, live arrivals, filters and turn navigation against the fake world. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test.beforeEach(async () => { await scenario(); });

test('new requests wait under a count while paused and resume without losing the turn link', async ({ page, request }) => {
  await signIn(page);
  await page.goto('/usage?view=requests&range=7d');
  const log = page.getByRole('table', { name: 'Requests', exact: true });
  await expect(log.getByRole('row').nth(1)).toBeVisible();
  const first = await log.getByRole('link').first().getAttribute('href');
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  expect((await request.post(`${DOUBLE}/__double/usage/use`, { data: { pattern: 'A', program: 'research' } })).ok()).toBe(true);
  await expect(page.getByRole('button', { name: '1 new', exact: true })).toBeVisible();
  await expect(log.getByRole('link').first()).toHaveAttribute('href', first ?? '');
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await expect(log.getByRole('row').nth(1)).toContainText('research');
  const filters = page.getByRole('button', { name: 'Filter requests', exact: true });
  if (await filters.isVisible()) await filters.click();
  await page.getByRole('combobox', { name: 'Access programme', exact: true }).click();
  await page.getByRole('option', { name: 'research', exact: true }).click();
  await expect(page).toHaveURL(/program=research/);
  await expect(log.getByRole('row')).toHaveCount(2);
  const turn = log.getByRole('link').first();
  await expect(turn).toHaveAttribute('href', /\/thread\/.+\?turn=1$/);
  await turn.click();
  await expect(page).toHaveURL(/\/thread\/.+\?turn=1$/);
});

test('the three views keep their span and filters in the same address', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage?view=requests&range=7d&program=research');
  const tabs = page.getByRole('tablist', { name: 'Usage views', exact: true });
  await tabs.getByRole('tab', { name: 'Performance', exact: true }).click();
  await expect(page).toHaveURL(/view=performance/);
  await expect(page).toHaveURL(/range=7d/);
  await expect(page).toHaveURL(/program=research/);
  await expect(page.getByRole('list', { name: 'Performance figures' })).toContainText('—');
  await tabs.getByRole('tab', { name: 'Consumption', exact: true }).click();
  await expect(page).toHaveURL(/program=research/);
});
