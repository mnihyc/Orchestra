/** Actual fleet selection and the environment's native readiness share their existing plans and controls. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, RUNS_ON } from '../doubles/fixtures.ts';
import { World } from '../doubles/world.ts';
import { sandboxReadiness } from '../fixtures/sandbox-readiness.ts';
import { change } from './change-support.ts';
import { emit, scenario } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`moves the selected fleet through the native node review ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await change.mode('manual'); await change.host({ id: RUNS_ON.buildHost, schedulable: true });
    await emit(ACME.mainThread, { thread: { state: 'idle' } });
    await change.environment({ id: RUNS_ON.sandbox, manages: ['ada'] });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto('/resources/environments');
    for (const name of ['Workspace', 'Sandbox']) {
      const row = page.getByRole('row').filter({ has: page.getByRole('link', { name, exact: true }) });
      await row.getByRole('checkbox').check();
    }
    await page.getByRole('button', { name: caption('@change.env.menu.move', locale), exact: true }).click();
    const form = page.getByRole('dialog', { name: caption('@fleetMoves.title', locale), exact: true });
    await form.getByRole('checkbox', { name: caption('@changeCompact.copyDirectories', locale), exact: true }).check();
    const confirm = form.getByRole('button', { name: caption(locale === 'en' ? '@envTransfer.changeCount_other' : '@envTransfer.changeCount', locale).replace('{{number}}', '2'), exact: true });
    await expect(confirm).toBeEnabled(); await expect(confirm).not.toHaveAttribute('aria-disabled', 'true');
    await expect(form.getByRole('table')).toContainText('Workspace'); await expect(form.getByRole('table')).toContainText('Sandbox');
    await page.screenshot({ path: info.outputPath('fleet-native-move-plan.png') });
    const submitted = page.waitForRequest((request) => request.method() === 'POST' && /\/nodes\/[^/]+\/environments\/moves$/.test(new URL(request.url()).pathname));
    await confirm.click();
    const body: unknown = (await submitted).postDataJSON();
    if (typeof body !== 'object' || body === null || !('preflight_id' in body)) throw new Error('The saved native plan confirmation is required');
    expect(typeof body.preflight_id).toBe('string');
    expect(body).toHaveProperty('notify', false); expect(body).not.toHaveProperty('environment_ids');
    await change.finish();
    await expect(form.getByText(caption('@fleetMoves.done', locale), { exact: true })).toBeVisible();
  });

  test(`shows native sandbox needs-attention and opens its existing recovery ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    const world = new World(), ada = world.people.get('ada');
    if (ada === undefined) throw new Error('The native reader is required');
    const native = world.resources.inventoryView(ada, 'environment', RUNS_ON.workspace);
    if (native?.kind !== 'environment') throw new Error('The full native environment projection is required');
    const report = sandboxReadiness(native.value, native.value.machine);
    await page.route(`**/environments/${RUNS_ON.workspace}`, async (route) => {
      const response = await route.fetch();
      if (!response.ok()) { await route.fulfill({ response }); return; }
      const actual: unknown = await response.json();
      if (typeof actual !== 'object' || actual === null || !('id' in actual) || actual.id !== native.value.id) throw new Error('The authorized native environment read is required');
      // Supply a typed capability report only after the actual ordinary read succeeds.
      await route.fulfill({ response, json: { ...actual, sandbox_readiness: report } });
    });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
    const title = caption('@sandboxReadiness.blocked', locale).replace('{{name}}', native.value.name);
    const notice = page.getByRole('alert').filter({ hasText: title });
    await expect(notice).toBeVisible();
    await expect(notice.getByRole('link', { name: caption('@sandboxReadiness.openEnvironment', locale), exact: true })).toHaveCount(0);
    await notice.getByRole('button', { name: caption('@help.hint.label', locale).replace('{{term}}', title), exact: true }).click();
    await expect(page.getByRole('dialog', { name: title, exact: true })).toContainText(caption('@sandboxReadiness.modes.workspace.name', locale));
    await page.keyboard.press('Escape');
    await page.screenshot({ path: info.outputPath('environment-native-readiness.png') });
    await notice.getByRole('button', { name: caption('@change.env.menu.move', locale), exact: true }).click();
    await expect(page.getByRole('dialog').filter({ has: page.locator('[data-change-destination]') })).toBeVisible();
  });
}
