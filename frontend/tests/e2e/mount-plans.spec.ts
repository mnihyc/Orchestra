/** A change wizard names a detached mount and its reason before a person confirms it. */
import { expect, test } from '@playwright/test';

import { change, openBlocked, wizard } from './change-support.ts';
import { scenario } from './support.ts';

test('the reviewed change says the target has no location for a mount', async ({ page }) => {
  await scenario();
  await change.mode('manual');
  await page.route('**/forks/preflight', async (route) => {
    const response = await route.fetch();
    const plan = await response.json() as Record<string, unknown>;
    plan.mounts = [{ mount: { id: 'mount-data', name: 'dataset', address: 'team.dataset' }, action: 'detach', path: null, mode: null, detached_because: 'no_location',
      fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged', copy: null, data_kind: null }];
    await route.fulfill({ response, json: plan });
  });
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByText('dataset will be detached: the target machine has no location for it')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeVisible();
});
