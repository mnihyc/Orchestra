/** Recent uses the same reader state in the bar, phone sheet and wide/narrow inventory. */
import { writeFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import type { ThreadDisplayState } from '../../src/generated/api-types';
import { display } from '../doubles/thread-display.ts';
import { ACME, emit, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;
const STATES = ['running', 'waiting_approval', 'blocked', 'failed', 'queued', 'held', 'finished'] as const;

async function setState(state: ThreadDisplayState) {
  await emit(MAIN, { thread: {
    state: state === 'running' ? 'running' : state === 'waiting_approval' ? 'waiting_approval' : state === 'blocked' ? 'blocked' : 'idle',
    blocked: state === 'blocked' ? { reason: 'provider_unavailable', detail: null } : null,
    pending_approvals: 0,
    display: display(state, { blocked_reason: state === 'blocked' ? 'provider_unavailable' : null,
      queued_inputs: state === 'queued' ? 2 : 0, held_inputs: state === 'held' ? 2 : 0,
      finished_at: state === 'finished' ? new Date().toISOString() : null }),
  } });
}

async function navigation(page: Page, width: number) {
  const bar = page.locator('[data-workbench-bar="left"]:visible');
  if (width === 400) {
    const toggle = page.locator('[data-container-choice="left:navigation"]');
    await expect(toggle).toBeVisible();
    if (!await bar.isVisible()) await toggle.click();
  } else {
    const toggle = page.getByRole('button', { name: 'Left bar', exact: true });
    await expect(toggle).toBeVisible();
    if (!await bar.isVisible()) await toggle.click();
  }
  await expect(bar).toBeVisible();
  return bar;
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`Recent shows reader states at ${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    await setState('idle');
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await signIn(page);
    await page.goto(`/thread/${MAIN}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    await page.goto('/org/acme');
    await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
    await page.waitForLoadState('networkidle');
    let bar = await navigation(page, width);
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Recent', exact: true }).click();
    const row = bar.locator(`[data-tree-row="thread:${MAIN}"]`);
    await expect(row).toBeVisible();
    for (const state of STATES) {
      await setState(state);
      await expect(row.locator(`[data-work-state="${state}"]`)).toBeVisible();
      if (state === 'held' || state === 'finished') await page.screenshot({ path: info.outputPath(`bar-${state}.png`) });
    }
    const reads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && (path === `/api/v1/nodes/${MAIN}` || path.startsWith(`/api/v1/threads/${MAIN}/`))) reads.push(path);
    });
    for (const state of STATES) {
      await setState(state);
      await page.goto('/everything?arrangement=recent&order=opened');
      const card = page.locator(`[data-everything-row="thread:${MAIN}"]`);
      await expect(card).toBeVisible();
      await expect(card.locator('.flat-name [data-node-icon="thread"]')).toBeVisible();
      await expect(card.locator(`.flat-state [data-work-state="${state}"]`)).toBeVisible();
      if (state === 'held' || state === 'finished') await page.screenshot({ path: info.outputPath(`everything-${state}.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    expect(reads).toEqual([]);
    expect(errors).toEqual([]);
    await writeFile(info.outputPath('icon-reads.json'), JSON.stringify(reads));
    bar = await navigation(page, width);
    await expect(bar.locator(`[data-tree-row="thread:${MAIN}"] [data-work-state="finished"]`)).toBeVisible();
  });
}
