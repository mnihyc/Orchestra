/** A requested mount comparison keeps its server time, never scans on ordinary plans, and fits the existing row. */
import { expect, test } from '@playwright/test';
import type { EnvironmentMovePlan, ForkPlan, PlanMount } from '../../src/generated/api-types';
import { id as fixtureId, KIND, RUNS_ON } from '../doubles/fixtures.ts';
import { BUILD_HOST, change, conversation, openHistory, shot, wizard } from './change-support.ts';
import { scenario, signIn } from './support.ts';

test('a delayed check cannot recreate a plan after its destination changes', async ({ page }) => {
  await scenario();
  await change.host({ id: BUILD_HOST, schedulable: true });
  let automaticPlans = 0;
  let release: () => void = () => undefined;
  const held = new Promise<void>((resolve) => { release = resolve; });
  await page.route('**/moves/preflight', async (route) => {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    if (body.check_fingerprints === true) await held;
    else if (body.host_id === undefined) automaticPlans += 1;
    const response = await route.fetch();
    const plan = await response.json() as EnvironmentMovePlan;
    plan.mounts = [{ mount: { id: 'synced-data', name: 'datasets', address: 'team.datasets' }, action: 'keep', path: body.host_id === BUILD_HOST ? '/mnt/checked-destination' : '/mnt/datasets', mode: 'read_only',
      detached_because: null, fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged', copy: null, data_kind: 'own',
      fingerprints_available: true, fingerprints: body.check_fingerprints === true ? { checked_at: new Date().toISOString(), matches: true } : null }];
    await route.fulfill({ response, json: plan });
  });
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
  await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
  await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
  const dialog = wizard(page, 'Move Sandbox');
  const row = dialog.getByRole('row').filter({ has: page.locator('[data-mount-fate="datasets"]') });
  await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeEnabled();
  expect(automaticPlans).toBe(1);
  try {
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row.getByRole('button', { name: 'Checking…', exact: true })).toBeVisible();
    const destinationRead = page.waitForResponse((response) => {
      if (!response.url().endsWith('/moves/preflight')) return false;
      const body = response.request().postDataJSON() as Record<string, unknown>;
      return body.host_id === BUILD_HOST && body.check_fingerprints !== true;
    });
    await dialog.getByRole('combobox', { name: 'Place', exact: true }).click();
    await page.getByRole('option', { name: 'build-host', exact: true }).click();
    await destinationRead;
    await expect(dialog.getByRole('region', { name: 'Changes', exact: true })).toContainText('/mnt/checked-destination');
  } finally { release(); }
  await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeEnabled();
  await expect(row.getByRole('status')).toHaveCount(0);
  await dialog.getByRole('combobox', { name: 'Place', exact: true }).click();
  await page.getByRole('option', { name: /^Automatic/u }).click();
  await expect.poll(() => automaticPlans).toBe(2);
  await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeEnabled();
  await expect(row.getByRole('status')).toHaveCount(0);
});

for (const width of [1280, 390]) for (const kind of ['move', 'fork'] as const) {
  test(`${kind} checks synced content only on request (${width}px)`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await change.host({ id: BUILD_HOST, schedulable: true });
    const bodies: Record<string, unknown>[] = [];
    let checks = 0;
    let fail = false;
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => { release = resolve; });
    const checkedAt = new Date().toISOString();
    await page.route(kind === 'move' ? '**/moves/preflight' : '**/forks/preflight', async (route) => {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      bodies.push(body);
      if (body.check_fingerprints === true) {
        checks += 1;
        if (checks === 1) await held;
        if (fail) {
          await route.fulfill({ status: 409, json: { type: 'orch:error:plan.blocked', code: 'plan.blocked', title: 'Blocked', status: 409, detail: 'The check failed', request_id: 'content-check', blockers: [{ code: 'mount_fingerprint_unavailable', message: 'The data host cannot read this location' }] } });
          return;
        }
      }
      const response = await route.fetch();
      const plan = await response.json() as ForkPlan | EnvironmentMovePlan;
      const mount: PlanMount = {
        mount: { id: 'synced-data', name: 'datasets', address: 'team.datasets' }, action: 'keep', path: '/mnt/datasets', mode: 'read_only',
        detached_because: null, fate: 'as_is', default_fate: 'as_is', fates: kind === 'move' ? ['detach', 'as_is', 'copy'] : ['as_is'], copy_unavailable: kind === 'move' ? null : 'unchanged', copy: null,
        data_kind: 'own', fingerprints_available: true,
        fingerprints: body.check_fingerprints === true ? { checked_at: checkedAt, matches: checks === 1 } : null,
      };
      plan.mounts = [mount];
      await route.fulfill({ response, json: plan });
    });
    if (kind === 'move') {
      await signIn(page);
      await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
      await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
      await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
    } else {
      await openHistory(page);
      await conversation(page).getByText('The handler reads').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Fork from here…' }).click();
    }
    const dialog = wizard(page, kind === 'move' ? 'Move Sandbox' : 'Fork “Main thread”');
    const row = dialog.getByRole('row').filter({ has: page.locator('[data-mount-fate="datasets"]') });
    await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeVisible();
    expect(checks).toBe(0);
    expect(bodies.every((body) => !('check_fingerprints' in body))).toBe(true);
    try {
      await row.getByRole('button', { name: 'Check', exact: true }).click();
      await expect(row.getByRole('button', { name: 'Checking…', exact: true })).toHaveAttribute('aria-busy', 'true');
      await shot(page, `mount-check-${kind}-pending-${width}`);
    } finally { release(); }
    await expect(row.getByRole('status')).toContainText('Same content · checked');
    await expect(row.locator('time')).toHaveAttribute('datetime', checkedAt);
    expect(checks).toBe(1);
    await shot(page, `mount-check-${kind}-same-${width}`);
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row.getByRole('status')).toContainText('Differs · checked');
    expect(checks).toBe(2);
    await shot(page, `mount-check-${kind}-differs-${width}`);
    fail = true;
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row.getByRole('alert')).toHaveText('Could not check · The data host cannot read this locationCheck');
    await expect(row.getByRole('status')).toHaveCount(0);
    expect(checks).toBe(3);
    const failureFits = await row.getByRole('alert').evaluate((element) => {
      const copy = element.querySelector('[data-state-copy]');
      const action = element.querySelector('[data-state-action]');
      if (copy === null || action === null) return false;
      const range = document.createRange();
      range.selectNodeContents(copy);
      const text = range.getBoundingClientRect();
      const button = action.getBoundingClientRect();
      const alert = element.getBoundingClientRect();
      return text.left >= alert.left && text.right <= alert.right
        && (text.right <= button.left || text.left >= button.right || text.bottom <= button.top || text.top >= button.bottom);
    });
    expect(failureFits, 'the read failure stays inside its row and clear of Check').toBe(true);
    await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeInViewport();
    await shot(page, `mount-check-${kind}-failed-${width}`);
    fail = false;
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row.getByRole('status')).toContainText('Differs · checked');
    expect(checks).toBe(4);
    await expect(row.locator('[data-fingerprint-check]')).toHaveCSS('flex-wrap', 'wrap');
    if (kind === 'fork') {
      await dialog.getByRole('combobox', { name: 'Environment', exact: true }).click();
      await page.getByRole('option', { name: 'Copy to new', exact: true }).click();
    } else {
      await dialog.getByRole('combobox', { name: 'Place', exact: true }).click();
      await page.getByRole('option', { name: 'build-host', exact: true }).click();
    }
    await expect(row.getByRole('status')).toHaveCount(0);
    expect(checks).toBe(4);
    expect(bodies.at(-1)).not.toHaveProperty('check_fingerprints');
  });
}

for (const width of [1280, 390]) for (const kind of ['move', 'fork'] as const) {
  test(`${kind} keeps live copy counts and a cancellable content check in its mount rows (${width}px)`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await change.host({ id: BUILD_HOST, schedulable: true });
    const firstId = fixtureId(KIND.job, 501);
    const retryId = fixtureId(KIND.job, 502);
    const countId = fixtureId(KIND.job, 503);
    const startedAt = new Date().toISOString();
    const oldUpdate = new Date(Date.now() - 31_000).toISOString();
    let check = await change.measurement({ id: firstId, phase: 'fingerprint_source', files: 7, bytes: 4096, startedAt, updatedAt: oldUpdate, stall: 30 });
    let count = await change.measurement({ id: countId, phase: 'measure', files: 3, bytes: 8192, startedAt });
    const checkedAt = new Date().toISOString();
    const retried = await change.measurement({ id: retryId, phase: 'fingerprint_target', state: 'succeeded', matches: false, checkedAt });
    const requests = new Set<string>();
    let automatic = 0;
    let jobHttpReads = 0;
    page.on('request', (request) => { if (request.method() === 'GET' && request.url().includes('/jobs/')) jobHttpReads += 1; });
    await page.route(kind === 'move' ? '**/moves/preflight' : '**/forks/preflight', async (route) => {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      let observed = check;
      if (body.check_fingerprints === true) {
        expect(typeof body.fingerprint_request_id).toBe('string');
        const request = String(body.fingerprint_request_id);
        requests.add(request);
        if (requests.size > 1) observed = retried;
      } else automatic += 1;
      const response = await route.fetch();
      const plan = await response.json() as ForkPlan | EnvironmentMovePlan;
      const checked = body.check_fingerprints === true;
      plan.mounts = [
        { mount: { id: fixtureId(0x32, 3), name: 'datasets', address: 'team.datasets' }, action: 'keep', path: '/mnt/datasets', mode: 'read_only',
          detached_because: null, fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged', copy: null, data_kind: 'own', fingerprints_available: true,
          fingerprints: checked && observed.state === 'succeeded' ? { matches: requests.size === 1, checked_at: observed.finished_at ?? checkedAt } : null,
          ...(checked ? { fingerprint_measurement: { job_id: observed.id, measured_at: observed.state === 'succeeded' ? observed.finished_at : null } } : {}) },
        { mount: { id: fixtureId(0x32, 4), name: 'copied files', address: 'team.copied' }, action: 'attach', path: '/mnt/copied', mode: 'read_only',
          detached_because: null, fate: 'copy', default_fate: 'copy', fates: ['detach', 'as_is', 'copy'], copy_unavailable: null, data_kind: 'own', fingerprints_available: false, fingerprints: null,
          copy: { measured: count.state === 'succeeded', file_count: count.progress.files_done, total_bytes: count.progress.bytes_done,
            measurement: { job_id: count.id, measured_at: count.finished_at }, location: { kind: 'volume', made: true, source: 'orch-mount-copy-target', storage: null }, holds: null, into: null, replace: false } },
      ];
      await route.fulfill({ response, json: plan });
    });
    if (kind === 'move') {
      await signIn(page);
      await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
      await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
      await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
    } else {
      await openHistory(page);
      await conversation(page).getByText('The handler reads').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Fork from here…' }).click();
    }
    const dialog = wizard(page, kind === 'move' ? 'Move Sandbox' : 'Fork “Main thread”');
    const row = dialog.getByRole('row').filter({ has: page.locator('[data-mount-fate="datasets"]') });
    const copied = dialog.getByRole('row').filter({ has: page.locator('[data-mount-fate="copied files"]') });
    await expect(copied).toContainText('Counting… 3 files · 8 KiB');
    await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeVisible();
    expect(requests.size).toBe(0);
    expect(automatic).toBeGreaterThan(0);
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row).toContainText('Checking source · 7 files · 4 KiB');
    await expect(row.locator('[data-job-stalled]')).toBeVisible();
    await expect(row.getByRole('button', { name: 'Checking…', exact: true })).toBeDisabled();
    await row.scrollIntoViewIfNeeded();
    await shot(page, `mount-live-${kind}-source-stalled-${width}`);
    check = await change.measurement({ id: firstId, phase: 'fingerprint_target', files: 11, bytes: 16384, startedAt });
    await expect(row).toContainText('Checking destination · 11 files · 16 KiB');
    await expect(row.locator('[data-job-stalled]')).toHaveCount(0);
    await shot(page, `mount-live-${kind}-target-${width}`);
    await row.getByRole('button', { name: 'Stop checking', exact: true }).click();
    await expect(row).toContainText('Stopping…');
    await expect(row.getByRole('button', { name: 'Stopping…', exact: true })).toBeDisabled();
    await shot(page, `mount-live-${kind}-stopping-${width}`);
    check = await change.measurement({ id: firstId, phase: 'fingerprint_target', state: 'cancelled', startedAt });
    await expect(row).toContainText('Check stopped');
    await expect(row.getByRole('button', { name: 'Check', exact: true })).toBeEnabled();
    await expect(row.getByRole('status')).not.toContainText('Same content');
    await shot(page, `mount-live-${kind}-stopped-${width}`);
    count = await change.measurement({ id: countId, phase: 'measure', state: 'succeeded', files: 12, bytes: 12288, startedAt });
    await expect(copied).toContainText('12 files · 12 KiB');
    await expect(row).toContainText('Check stopped');
    expect(requests.size).toBe(1);
    await row.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(row.getByRole('status')).toContainText('Differs · checked');
    await expect(row.locator('time')).toHaveAttribute('datetime', checkedAt);
    expect(requests.size).toBe(2);
    expect(jobHttpReads).toBe(0);
    await shot(page, `mount-live-${kind}-retried-${width}`);
  });
}
