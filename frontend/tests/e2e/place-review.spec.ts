/** Selected rows are reviewed together but keep separate fences, progress, refusals, observed lost answers and Undo. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

async function select(page: Page, names: readonly string[]) {
  await page.goto(`/project/${ACME.web}?view=list&status=all`);
  for (const name of names) await page.getByRole('checkbox', { name: `Select ${name}`, exact: true }).check();
}

test('a phone review names the archive work, reports a partial refusal and lost answer, and undoes the confirmed row alone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await select(page, ['Fix login', 'Upgrade deps', 'Migrate CI']);
  const applied: string[] = [];
  const undone: string[] = [];
  await page.route('**/api/v1/nodes/*/archive', async (route) => {
    const id = route.request().url().split('/').at(-2) ?? '';
    const body: unknown = route.request().postDataJSON();
    expect(body).toMatchObject({ preflight_id: expect.any(String), stamp: expect.any(String), expected_revision: expect.any(Number) });
    applied.push(id);
    if (id === ACME.upgradeDeps) await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Permission changed', detail: 'This selected topic no longer permits archive.', request_id: 'one-refusal' } });
    else if (id === ACME.migrateCi) { await route.fetch(); await route.abort('connectionreset'); }
    else await route.continue();
  });
  await page.route('**/api/v1/nodes/*/unarchive', async (route) => { undone.push(route.request().url().split('/').at(-2) ?? ''); await route.continue(); });
  await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Archive', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Review selected places', exact: true });
  await expect(dialog.getByRole('button', { name: 'Apply to 3 ready places' })).toBeEnabled();
  await expect(dialog.getByRole('list', { name: 'Covered subtree' })).toHaveCount(3);
  await expect(dialog.getByRole('list', { name: 'Work that stops' })).toHaveCount(3);
  expect(applied).toEqual([]);
  await dialog.getByRole('button', { name: 'Apply to 3 ready places' }).click();
  const result = page.getByRole('dialog', { name: 'Results for selected places', exact: true });
  await expect(result.getByText('Answer lost; outcome unknown', { exact: true })).toBeVisible();
  await expect(result.getByText('Some rows did not succeed. Each result below stands on its own.', { exact: true })).toBeVisible();
  const lost = result.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Migrate CI', exact: true }) });
  await lost.getByRole('button', { name: 'Read current state', exact: true }).click();
  await expect(lost).toContainText('Read only afterwards');
  await result.getByRole('button', { name: 'Undo successful rows', exact: true }).click();
  await expect(result.getByRole('heading', { name: 'Undo results' })).toBeVisible();
  expect(applied).toHaveLength(3);
  expect(undone).toEqual([ACME.fixLogin]);
});

test('a changed subtree is refused and only a new displayed review can be accepted', async ({ page }) => {
  await signIn(page);
  await select(page, ['Upgrade deps']);
  await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Archive', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review selected places', exact: true });
  await expect(review.getByRole('button', { name: 'Apply to 1 ready place' })).toBeEnabled();
  // The browser reports the server's changed-subtree refusal and waits for a fresh review.
  await page.route(`**/api/v1/nodes/${ACME.upgradeDeps}/archive`, async (route) => {
    await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Review changed', detail: 'A newer child changed this archive review.', request_id: 'stale-subtree', current: { revision: 2 } } });
  }, { times: 1 });
  await review.getByRole('button', { name: 'Apply to 1 ready place' }).click();
  const results = page.getByRole('dialog', { name: 'Results for selected places', exact: true });
  await expect(results.getByText('Refused', { exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Review again', exact: true }).click();
  await expect(review.getByRole('button', { name: 'Apply to 1 ready place' })).toBeEnabled();
  await review.getByRole('button', { name: 'Apply to 1 ready place' }).click();
  await expect(results.getByText('Succeeded', { exact: true })).toBeVisible();
});

test('a selected deletion keeps its own recovery group and the aggregate Undo restores only that group', async ({ page }) => {
  await signIn(page);
  await select(page, ['Upgrade deps']);
  await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Delete', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review selected places', exact: true });
  await expect(review).toContainText('Who may bring it back');
  await expect(review).toContainText('Gone for good');
  await review.getByRole('button', { name: 'Apply to 1 ready place' }).click();
  const results = page.getByRole('dialog', { name: 'Results for selected places', exact: true });
  await expect(results.getByText('Succeeded', { exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Undo successful rows', exact: true }).click();
  await expect(results.getByRole('heading', { name: 'Undo results', exact: true })).toBeVisible();
  await expect(results.getByText('Succeeded', { exact: true })).toHaveCount(2);
  await results.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: 'Select Upgrade deps', exact: true })).toBeVisible();
});

test('a selected move shows its destination and consequences and Undo returns the confirmed revision to its former project', async ({ page }) => {
  await signIn(page);
  await select(page, ['Upgrade deps']);
  await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Move', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review selected places', exact: true });
  await review.getByRole('combobox', { name: 'Move to', exact: true }).fill('Evals');
  await page.getByRole('option', { name: /Evals/ }).click();
  await expect(review.getByRole('button', { name: 'Apply to 1 ready place' })).toBeEnabled();
  await expect(review).toContainText('From');
  await expect(review.getByRole('link', { name: 'Evals', exact: true })).toBeVisible();
  const confirmations: unknown[] = [];
  await page.route(`**/api/v1/topics/${ACME.upgradeDeps}/move`, async (route) => { confirmations.push(route.request().postDataJSON() as unknown); await route.continue(); });
  await review.getByRole('button', { name: 'Apply to 1 ready place' }).click();
  const result = page.getByRole('dialog', { name: 'Results for selected places', exact: true });
  await expect(result.getByText('Succeeded', { exact: true })).toBeVisible();
  await result.getByRole('button', { name: 'Undo successful rows', exact: true }).click();
  await expect(result.getByText('Undone', { exact: true })).toBeVisible();
  expect(confirmations).toEqual([
    { parent_id: ACME.evals, expected_revision: expect.any(Number), preflight_id: expect.any(String) },
    { parent_id: ACME.web, expected_revision: expect.any(Number), preflight_id: expect.any(String) },
  ]);
});
