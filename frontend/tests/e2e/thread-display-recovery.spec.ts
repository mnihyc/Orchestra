/** Automatic retry waits and stops agree in list, tab and header icons on the shipping frontend. */
import { writeFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import type { CallRecovery } from '../../src/generated/api-types';
import { display, recoveryDisplay } from '../doubles/thread-display.ts';
import { ACME, emit, scenario, signIn } from './support.ts';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`recovery icons show scheduled and stopped at ${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({width, height: 1000});
    await page.emulateMedia({colorScheme: theme});
    const due = new Date(Date.now() + 55_000).toISOString();
    let recovery: CallRecovery = {state: 'scheduled', failure: {code: 'provider_busy', message: 'Synthetic capacity', retryable: true,
      window: null, said: null, resets_at: null, incomplete: false}, rule_id: 'busy', step: {type: 'retry', times: 3, delay_secs: 60, backoff: 2},
      attempt: 2, due_at: due, detail: null, count: null, window_secs: null};
    let detailReads = 0;
    await page.route(`**/api/v1/threads/${ACME.mainThread}/calls/recovery`, async (route) => {
      detailReads++;
      await route.fulfill({json: {recovery}});
    });
    await emit(ACME.mainThread, {thread: {state: 'blocked', current_turn_id: null, lock: null,
      blocked: {reason: 'error', detail: 'call_recovery', sandbox: null},
      display: display('queued', {blocked_reason: 'error', recovery: recoveryDisplay({due_at: due})})}});
    await signIn(page);
    const reads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && (path === `/api/v1/nodes/${ACME.mainThread}` || path.startsWith(`/api/v1/threads/${ACME.mainThread}/`))) reads.push(path);
    });
    await page.goto('/everything?arrangement=recent&order=active');
    const row = page.locator(`[data-everything-row="thread:${ACME.mainThread}"]`);
    await expect(row.locator('[data-work-state="queued"]')).toBeVisible();
    await expect(row.getByRole('img', {name: /Retrying in .* · attempt 2 of 3/})).toBeVisible();
    expect(reads).toEqual([]);
    expect(detailReads).toBe(0);
    const listIconReads = {scheduled: [...reads], stopped: [] as string[]};
    await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]');
    await expect(header.getByRole('img', {name: /Retrying in .* · attempt 2 of 3/})).toBeVisible();
    await expect(header.locator('svg.lucide-clock-3')).toBeVisible();
    await expect(page.getByRole('tab').filter({hasText: 'Main thread'}).locator('svg.lucide-clock-3')).toBeVisible();
    expect(await header.locator('svg.lucide-clock-3').evaluate((icon) => getComputedStyle(icon).animationName)).toBe('none');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({path: info.outputPath('scheduled.png')});
    recovery = {...recovery, state: 'stopped', step: null, due_at: null, detail: 'repeat_limit', count: 4, window_secs: 300};
    await emit(ACME.mainThread, {thread: {display: display('failed', {blocked_reason: 'error', recovery: recoveryDisplay({step: null,
      attempts: null, due_at: null, stop_reason: 'repeat_limit', count: 4, window_secs: 300})})}});
    await expect(header.getByRole('img', {name: 'Provider busy · 4 times in 5 min', exact: true})).toBeVisible();
    await expect(header.locator('svg.lucide-octagon-alert')).toBeVisible();
    await expect(page.getByRole('tab').filter({hasText: 'Main thread'}).locator('svg.lucide-octagon-alert')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({path: info.outputPath('stopped.png')});
    await page.waitForLoadState('networkidle');
    reads.length = 0;
    if (width === 400) {
      await page.getByRole('button', {name: 'Find tab in Main area', exact: true}).click();
      await page.getByRole('option', {name: /^Everything/}).click();
    } else await page.getByRole('tab', {name: 'Everything', exact: true}).click();
    await page.getByRole('button', {name: 'Refresh', exact: true}).click();
    await expect(row.locator('[data-work-state="failed"]')).toBeVisible();
    await expect(row.getByRole('img', {name: 'Provider busy · 4 times in 5 min', exact: true})).toBeVisible();
    await page.waitForLoadState('networkidle');
    listIconReads.stopped = [...reads];
    expect(listIconReads.stopped).toEqual([]);
    await writeFile(info.outputPath('requests.json'), JSON.stringify({listIconReads, detailReads}, null, 2));
  });
}
