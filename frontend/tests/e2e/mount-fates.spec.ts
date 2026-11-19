/** A move offers each mount Detach, Mount as-is or Copy, preselected to its default, and the plan follows the choice. */
import { expect, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { BUILD_HOST, change, shot, wizard } from './change-support.ts';
import { scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

for (const width of [1280, 390]) {
  test(`a move names each mount's fate and its plan follows a copy or a detach (${width}px)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await change.host({ id: BUILD_HOST, schedulable: true });
    await signIn(page);
    await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
    await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
    await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
    const dialog = wizard(page, 'Move Sandbox');
    await dialog.getByRole('combobox', { name: 'Place', exact: true }).click();
    await page.getByRole('option', { name: 'build-host', exact: true }).click();
    const datasets = dialog.getByRole('combobox', { name: 'What becomes of datasets' });
    await expect(datasets).toContainText('Mount as-is');
    await expect(dialog.getByText('datasets will be detached: the target machine has no location for it')).toBeVisible();
    await shot(page, `mount-fates-1-as-is-${width}`);
    await datasets.click();
    await page.getByRole('option', { name: 'Copy', exact: true }).click();
    await expect(dialog.getByText(/^The data of datasets is copied/)).toBeVisible();
    await expect(datasets).toContainText('Copy');
    await shot(page, `mount-fates-2-copy-${width}`);
    await datasets.click();
    await page.getByRole('option', { name: 'Detach', exact: true }).click();
    await expect(dialog.getByText('datasets will be detached: it was chosen to be detached')).toBeVisible();
    await shot(page, `mount-fates-3-detach-${width}`);
  });
}
