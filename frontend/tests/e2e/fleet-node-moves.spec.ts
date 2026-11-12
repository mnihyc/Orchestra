/** Actual fleet selection and the environment's native readiness share their existing plans and controls. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, RUNS_ON } from '../doubles/fixtures.ts';
import { World } from '../doubles/world.ts';
import { sandboxReadiness } from '../fixtures/sandbox-readiness.ts';
import { change } from './change-support.ts';
import { bodySent, emit, scenario } from './support.ts';

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
    // The destination picker receives focus when the dialog opens. Choose the
    // target and an owned harness template before reviewing both environments.
    const destination = form.getByRole('combobox', { name: caption('@loss.destinationHost', locale), exact: true });
    await expect(destination).toBeVisible();
    if (await destination.getAttribute('aria-expanded') !== 'true') await destination.click();
    await page.getByRole('option', { name: 'build-host', exact: true }).click();
    await form.getByRole('combobox', { name: caption('@environments.make.template', locale), exact: true }).click();
    await page.getByRole('option', { name: 'Default', exact: true }).click();
    await form.getByRole('checkbox', { name: caption('@changeCompact.copyDirectories', locale), exact: true }).check();
    const confirm = form.getByRole('button', { name: caption('@envTransfer.changeCount_other', locale).replace('{{number}}', '2'), exact: true });
    await expect(confirm).toBeEnabled(); await expect(confirm).not.toHaveAttribute('aria-disabled', 'true');
    await expect(form.getByRole('table')).toContainText('Workspace'); await expect(form.getByRole('table')).toContainText('Sandbox');
    await page.screenshot({ path: info.outputPath('fleet-native-move-plan.png') });
    const submitted = page.waitForRequest((request) => request.method() === 'POST' && /\/nodes\/[^/]+\/environments\/moves$/.test(new URL(request.url()).pathname));
    await confirm.click();
    const body = await bodySent(await submitted);
    if (typeof body !== 'object' || body === null || !('preflight_id' in body)) throw new Error('The saved native plan confirmation is required');
    expect(typeof body.preflight_id).toBe('string');
    expect(body).toHaveProperty('notify', false); expect(body).not.toHaveProperty('environment_ids');
    await change.finish();
    // Completion names the progress indicator; its former duplicate text is gone.
    await expect(form.getByRole('progressbar', { name: caption('@fleetMoves.done', locale), exact: true })).toHaveAttribute('aria-valuenow', '100');
    await expect(form.getByRole('cell', { name: caption('@source.job.succeeded', locale), exact: true })).toHaveCount(2);
  });

  test(`shows native sandbox needs-attention and opens its existing recovery ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    const world = new World(), ada = world.people.get('ada');
    if (ada === undefined) throw new Error('The native reader is required');
    const native = world.resources.inventoryView(ada, 'environment', RUNS_ON.workspace);
    if (native?.kind !== 'environment') throw new Error('The full native environment projection is required');
    const report = sandboxReadiness(native.value, native.value.machine);
    // The native resource projection is also delivered by the personal feed;
    // supply its report at the fixture source so every authorized transport agrees.
    const supplied = await fetch(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/resources/environment-readiness`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ environment_id: native.value.id, readiness: report }),
    });
    expect(supplied.ok).toBe(true);
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
