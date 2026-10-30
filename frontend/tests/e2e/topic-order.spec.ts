/** One topic review reports every status result and inverse; inline tags distinguish definition from assignment. */
import { expect, test } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('selected rows have one review and one Undo even after Done leaves the Open list', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=list`);
  const box = page.getByRole('checkbox', { name: 'Select Fix login', exact: true });
  await box.check();
  const selection = page.getByLabel('Selected topics');
  await selection.getByRole('button', { name: 'Mark done', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Review 1 topics' });
  await expect(dialog.getByText('Fix login', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Mark 1 topics done', exact: true }).click();
  const results = page.getByRole('dialog', { name: 'Results for 1 topics' });
  await expect(results.getByText('Succeeded', { exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(results.getByRole('heading', { name: 'Undo results' })).toBeVisible();
  await expect(results.getByText('Fix login · Succeeded', { exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Close', exact: true }).last().click();
  await expect(page.getByRole('link', { name: 'Fix login', exact: true }).last()).toBeVisible();
});

test('an uncertain write is named and is never offered an inverse', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.route(`**/api/v1/topics/${ACME.fixLogin}`, async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fetch();
    await route.fulfill({ status: 502, json: { type: 'orch:error:internal', code: 'internal', title: 'The reply was lost', detail: 'The outcome is not confirmed.', status: 502, request_id: '' } });
  });
  await page.getByRole('checkbox', { name: 'Select Fix login', exact: true }).check();
  await page.getByLabel('Selected topics').getByRole('button', { name: 'Mark done', exact: true }).click();
  await page.getByRole('dialog', { name: 'Review 1 topics' }).getByRole('button', { name: 'Mark 1 topics done', exact: true }).click();
  const results = page.getByRole('dialog', { name: 'Results for 1 topics' });
  await expect(results.getByText('Not confirmed', { exact: true })).toBeVisible();
  await expect(results.getByRole('button', { name: 'Undo', exact: true })).toHaveCount(0);
  await results.getByRole('button', { name: 'Check current state', exact: true }).click();
  await expect(results.getByText(/Now Done/)).toBeVisible();
  await expect(results.getByRole('button', { name: 'Undo', exact: true })).toHaveCount(0);
});

test('a new tag uses its returned colour and survives assignment refusal with an explicit Apply retry', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: 'Edit tags', exact: true }).click();
  await page.getByLabel('Tag name').fill('infra');
  await page.route(`**/api/v1/topics/${ACME.fixLogin}`, async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Permission changed', detail: 'topic.interact is required.', status: 403, request_id: '' } });
  });
  await page.getByRole('button', { name: 'Make the tag infra', exact: true }).click();
  await expect(page.getByText('infra was created, but the topic was not changed.')).toBeVisible();
  await expect(page.getByLabel('Tag name')).toHaveValue('infra');
  await page.unroute(`**/api/v1/topics/${ACME.fixLogin}`);
  await page.getByRole('button', { name: 'Apply infra', exact: true }).click();
  await expect(page.getByLabel('Tag name')).toHaveValue('');
});

test('a partial batch names both outcomes and its one Undo reverses only the confirmed success', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.getByRole('checkbox', { name: 'Select Fix login', exact: true }).check();
  await page.getByRole('checkbox', { name: 'Select Upgrade deps', exact: true }).check();
  await page.route(`**/api/v1/topics/${ACME.fixLogin}`, async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Permission changed', detail: 'topic.interact is required.', status: 403, request_id: '' } });
  });
  await page.getByLabel('Selected topics').getByRole('button', { name: 'Mark done', exact: true }).click();
  await page.getByRole('dialog', { name: 'Review 2 topics' }).getByRole('button', { name: 'Mark 2 topics done', exact: true }).click();
  const results = page.getByRole('dialog', { name: 'Results for 2 topics' });
  await expect(results.getByText('Refused', { exact: true })).toBeVisible();
  await expect(results.getByText('Succeeded', { exact: true })).toBeVisible();
  await results.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(results.getByText('Upgrade deps · Succeeded', { exact: true })).toBeVisible();
  await expect(results.getByText('Fix login · Succeeded', { exact: true })).toHaveCount(0);
});

test('a newer edit refuses Undo and the ordinary list menu uses the same captured inverse', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=list&status=all`);
  const row = page.getByRole('row').filter({ has: page.getByRole('link', { name: 'Fix login', exact: true }) });
  await row.getByRole('button', { name: 'More about Fix login' }).click();
  await page.getByRole('menuitem', { name: 'Mark done', exact: true }).click();
  const changed = page.getByRole('region', { name: /^Notifications/ });
  await expect(changed.getByText('Fix login marked done', { exact: true })).toBeVisible();
  await page.route(`**/api/v1/topics/${ACME.fixLogin}`, async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Newer edit', detail: 'The topic changed after the status change.', status: 409, request_id: '' } });
  });
  await changed.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(changed.getByText('Fix login: Undo was refused', { exact: true })).toBeVisible();
});
