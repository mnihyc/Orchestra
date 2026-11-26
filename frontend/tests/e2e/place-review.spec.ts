/** Selected rows act at once and keep separate fences, refusals, lost answers and Undo; a selected deletion keeps its own review. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

const notices = (page: Page) => page.getByRole('region', { name: /^Notifications/ });

test.beforeEach(async () => { await scenario(); });

async function select(page: Page, names: readonly string[]) {
  await page.goto(`/project/${ACME.web}?view=list&status=all`);
  for (const name of names) await page.getByRole('checkbox', { name: `Select ${name}`, exact: true }).check();
}

test('a phone selection that stops running work says so in one line first, names a refusal and a lost answer by their rows, and Undo brings back the confirmed row alone', async ({ page }) => {
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
  // Fix login has a running turn and an approval waiting: that is the one thing said before the archive is made.
  const stops = page.getByRole('alertdialog', { name: 'Archive 3 topics?', exact: true });
  await expect(stops.getByText(/^Work running in \d+ threads? stops$/)).toBeVisible();
  await expect(stops.getByRole('list', { name: 'Covered subtree' })).toHaveCount(0);
  expect(applied).toEqual([]);
  await stops.getByRole('button', { name: 'Archive 3 topics', exact: true }).click();
  // The results name only the rows that did not succeed.
  const result = page.getByRole('dialog', { name: 'Results for 3 topics', exact: true });
  await expect(result.getByText('Upgrade deps', { exact: true })).toBeVisible();
  await expect(result.getByText('Migrate CI', { exact: true })).toBeVisible();
  await expect(result.getByText('This change was not confirmed, so Undo leaves it out. Check its current state before choosing again.', { exact: true })).toHaveCount(1);
  await expect(result.getByText('Fix login', { exact: true })).toHaveCount(0);
  expect(applied).toHaveLength(3);
  await result.getByRole('contentinfo').getByRole('button', { name: 'Close', exact: true }).click();
  await notices(page).getByRole('listitem').filter({ hasText: '1 topic archived' }).getByRole('button', { name: 'Undo', exact: true }).click();
  await expect.poll(() => undone).toEqual([ACME.fixLogin]);
});

test('a changed subtree refuses its row alone, and Archive again reads a new plan', async ({ page }) => {
  await signIn(page);
  await select(page, ['Upgrade deps']);
  await page.route(`**/api/v1/nodes/${ACME.upgradeDeps}/archive`, async (route) => {
    await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Review changed', detail: 'A newer child changed this archive review.', request_id: 'stale-subtree', current: { revision: 2 } } });
  }, { times: 1 });
  const bar = page.getByRole('region', { name: 'Selected topics' });
  await bar.getByRole('button', { name: 'Archive', exact: true }).click();
  const results = page.getByRole('dialog', { name: 'Results for 1 topic', exact: true });
  await expect(results.getByText('Upgrade deps', { exact: true })).toBeVisible();
  await results.getByRole('contentinfo').getByRole('button', { name: 'Close', exact: true }).click();
  await bar.getByRole('button', { name: 'Archive', exact: true }).click();
  await expect(notices(page).getByText('1 topic archived', { exact: true })).toBeVisible();
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
  await results.getByRole('button', { name: 'Undo what succeeded', exact: true }).click();
  await expect(results.getByRole('heading', { name: 'Undo results', exact: true })).toBeVisible();
  await expect(results.getByText('Succeeded', { exact: true })).toHaveCount(2);
  await results.getByRole('contentinfo').getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: 'Select Upgrade deps', exact: true })).toBeVisible();
});

test('a selected move goes at once to the chosen project and Undo returns it to its former project', async ({ page }) => {
  await signIn(page);
  await select(page, ['Upgrade deps']);
  await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Move', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Move 1 topic', exact: true });
  await dialog.getByRole('combobox', { name: 'Move to', exact: true }).fill('Evals');
  await page.getByRole('option', { name: /Evals/ }).click();
  const confirmations: unknown[] = [];
  await page.route(`**/api/v1/topics/${ACME.upgradeDeps}/move`, async (route) => { confirmations.push(route.request().postDataJSON() as unknown); await route.continue(); });
  await dialog.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const moved = notices(page).getByRole('listitem').filter({ hasText: /^1 topic moved to / });
  await expect(moved).toBeVisible();
  await moved.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(notices(page).getByText('1 topic moved back', { exact: true })).toBeVisible();
  expect(confirmations).toEqual([
    { parent_id: ACME.evals, expected_revision: expect.any(Number) },
    { parent_id: ACME.web, expected_revision: expect.any(Number) },
  ]);
});
