/** Streamed names and transcript results keep the same thread state without separate icon reads. */
import { writeFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { display } from '../doubles/thread-display.ts';
import { agentMessage } from '../doubles/thread-rows.ts';
import { ACME, emit, lookedAt, scenario, signIn } from './support.ts';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`streamed search shows reader states at ${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    await emit(ACME.mainThread, {items: [agentMessage('state-search-result', 'Main thread result for the state check')]});
    await page.setViewportSize({width, height: 1000});
    await page.emulateMedia({colorScheme: theme});
    await signIn(page);
    const reads: string[] = [];
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && (path === `/api/v1/nodes/${ACME.mainThread}` || path.startsWith(`/api/v1/threads/${ACME.mainThread}/`))) reads.push(path);
    });
    for (const state of ['running', 'waiting_approval', 'blocked', 'failed', 'queued', 'held', 'finished'] as const) {
      await emit(ACME.mainThread, {thread: {state: 'idle', display: display(state, {
        queued_inputs: state === 'queued' ? 2 : 0, held_inputs: state === 'held' ? 2 : 0,
        blocked_reason: state === 'blocked' ? 'provider_unavailable' : null,
      })}});
      await page.goto('/search?q=Main%20thread&sort=relevance');
      const results = page.getByRole('region', {name: 'Results', exact: true});
      const chips = results.locator(`a[title][href^="/thread/${ACME.mainThread}"]`);
      await expect(chips).toHaveCount(2);
      for (const chip of await chips.all()) await expect(chip.locator(`[data-work-state="${state}"]`)).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      if (state === 'held' || state === 'finished') await page.screenshot({path: info.outputPath(`search-${state}.png`)});
    }
    expect(reads).toEqual([]);
    expect(Object.values(await lookedAt()).flat()).toEqual([]);
    expect(errors).toEqual([]);
    await writeFile(info.outputPath('icon-reads.json'), JSON.stringify(reads));
  });
}
