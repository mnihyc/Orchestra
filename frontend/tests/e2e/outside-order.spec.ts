/** Outside Orch keeps the applicable lists together and a refused delivery switch says what happened. */
import { expect, test } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('the org, department, project and topic each show their applicable headed lists', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  for (const path of ['/org/acme/settings/channels', '/org/acme.research/settings/channels']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: 'Outside Orch', exact: true, level: 2 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'To people', exact: true, level: 3 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'To a room or a system', exact: true, level: 3 })).toBeVisible();
  }
  for (const path of [`/project/${ACME.web}/settings?section=destinations`, `/topic/${ACME.fixLogin}?tab=settings&section=destinations`]) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: 'Outside Orch', exact: true, level: 2 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'To a room or a system', exact: true, level: 3 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'To people', exact: true })).toHaveCount(0);
  }
});

test('a refused delivery switch returns to its confirmed state and keeps the refusal beside it', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}/settings?section=destinations`);
  const row = page.locator('[data-destination]').first();
  const delivery = row.getByRole('switch', { name: 'Enable delivery', exact: true });
  await expect(delivery).toBeChecked();
  await page.route('**/api/v1/destinations/*', async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Permission changed', detail: 'settings.manage is required.', status: 403, request_id: '' } });
  });
  await delivery.click();
  await expect(row.getByRole('alert')).toBeVisible();
  await expect(delivery).toBeChecked();
  await page.unroute('**/api/v1/destinations/*');
  await delivery.click();
  await expect(delivery).not.toBeChecked();
  await expect(row.getByRole('alert')).toHaveCount(0);
});
