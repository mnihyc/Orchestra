/** Accounted tokens are numeric metadata, not gigabytes of generated text or paid model requests. */
import { writeFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import type { ThreadOpening, ThreadSync } from '../../src/generated/api-types.ts';
import { ACME, delta, emit, scenario, signIn } from '../e2e/support.ts';
import { agentMessage, command, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';

const ROUNDS = 128;
const TOKENS = 10_000_000_000;
const ITEMS_PER_ROUND = 12;
// Same bounded initial window and ended-turn tail as the native thread contract.
const INITIAL_ITEMS = 200;
const ENDED_TAIL = 20;
// Reuse the existing thread-scale DOM/heap budgets; do not multiply them by machine load.
const ROWS = 90;
const NODES = 8_000;
const HEAP_BYTES = 400 * 1024 * 1024;

for (const [width, theme] of [[1440, 'light'], [400, 'dark']] as const) {
  test(`128 interactions and 10B accounted tokens retain bounded reads and live delivery at ${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    for (let round = 1; round <= ROUNDS; round += 1) {
      const id = `accounted-turn-${round}`;
      const input = `accounted-input-${round}`;
      const at = new Date(Date.UTC(2026, 9, 1, 10, round)).toISOString();
      const prompt = `Synthetic request ${round}`;
      await emit(ACME.mainThread, {
        turns: [turn(id, { ordinal: round, state: 'completed', inputId: input, tokens: TOKENS / ROUNDS, startedAt: at, completedAt: at })],
        inputs: [{ ...threadInput(input, prompt, { author: ADA, position: String(round), state: 'delivered' }), turn_id: id, delivered_at: at }],
        items: [
          userMessage(`accounted-user-${round}`, prompt, { author: ADA, turn: id, inputId: input, at }),
          ...Array.from({ length: ITEMS_PER_ROUND - 2 }, (_, step) => command(`accounted-tool-${round}-${step}`, {
            turn: id, at, command: `inspect module-${step}`, output: `Synthetic result ${round}/${step}. `.repeat(16), exit: 0,
          })),
          agentMessage(`accounted-answer-${round}`, `Synthetic result ${round} is complete.`, { turn: id, at }),
        ],
      });
    }
    const usage = { cached_input_tokens: TOKENS / 2, compact_at: null, context_used: 42_000, context_window: 200_000,
      input_tokens: TOKENS * 0.8, output_tokens: TOKENS * 0.2, reasoning_tokens: 0, total_tokens: TOKENS, updated_at: new Date().toISOString() };
    await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null, usage } });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    const errors: string[] = [];
    const reads: string[] = [];
    const snapshots: { path: string; bytes: number; items: number; turns: number; inputs: number; tokens: number | undefined }[] = [];
    const isHistoryRead = (path: string) => path.endsWith(`/threads/${ACME.mainThread}/opening`) || path.endsWith(`/threads/${ACME.mainThread}/sync`);
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path.startsWith('/api/v1/')) reads.push(path);
    });
    // Read the unchanged JSON before delivery. CDP may discard a response body
    // during a document navigation, which is not a product payload failure.
    await page.route(`**/api/v1/threads/${ACME.mainThread}/**`, async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (!isHistoryRead(path)) { await route.continue(); return; }
      const response = await route.fetch();
      const text = await response.text();
      if (response.ok()) {
        const body = path.endsWith('/opening') ? (JSON.parse(text) as ThreadOpening).sync : JSON.parse(text) as ThreadSync;
        snapshots.push({ path, bytes: new TextEncoder().encode(text).length, items: body.items.length, turns: body.turns.length,
          inputs: body.inputs.length, tokens: body.thread.usage?.total_tokens });
      }
      await route.fulfill({ response, body: text });
    });
    await signIn(page);
    const started = Date.now();
    await page.goto(`/thread/${ACME.mainThread}`);
    const conversation = page.getByRole('log', { name: 'Conversation', exact: true });
    await expect(conversation.getByText(`Synthetic result ${ROUNDS} is complete.`, { exact: true })).toBeVisible();
    const openMs = Date.now() - started;
    expect(snapshots).toHaveLength(1);
    expect(snapshots[0]?.tokens).toBe(TOKENS);
    expect(snapshots[0]?.items).toBeLessThanOrEqual(INITIAL_ITEMS);
    expect(snapshots[0]?.turns).toBeLessThanOrEqual(ENDED_TAIL + Math.ceil(INITIAL_ITEMS / ITEMS_PER_ROUND));
    expect(snapshots[0]?.inputs).toBe(0);
    // A bounded snapshot fits the existing four-MiB socket-frame ceiling too.
    expect(snapshots[0]?.bytes).toBeLessThan(4 * 1024 * 1024);
    await expect(page.locator('[data-thread-header]').getByText('Idle', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('accounted-latest.png'), fullPage: true, animations: 'disabled' });

    await page.goto(`/thread/${ACME.mainThread}?item=accounted-user-1`);
    await expect(conversation.getByText('Synthetic request 1', { exact: true })).toBeVisible();
    const latest = page.getByRole('button', { name: 'Jump to latest', exact: true });
    await expect(latest).toBeVisible();
    if (width === 1440) await latest.click();
    else { await conversation.focus(); await page.keyboard.press('End'); }
    await expect(conversation.getByText(`Synthetic result ${ROUNDS} is complete.`, { exact: true })).toBeVisible();
    const syncsBeforeLive = reads.filter(isHistoryRead).length;
    const at = new Date().toISOString();
    await emit(ACME.mainThread, {
      turns: [turn('accounted-live-turn', { ordinal: ROUNDS + 1, state: 'running', inputId: 'accounted-live-input', startedAt: at })],
      items: [userMessage('accounted-live-user', 'Keep going after the long history', { author: ADA, turn: 'accounted-live-turn', inputId: 'accounted-live-input', at }),
        agentMessage('accounted-live-answer', '', { turn: 'accounted-live-turn', status: 'in_progress', at })],
      thread: { state: 'running', current_turn_id: 'accounted-live-turn' },
    });
    await expect(conversation.getByText('Keep going after the long history', { exact: true })).toBeVisible();
    const sent = Date.now();
    const answer = 'Still responsive after 128 interactions.';
    await delta(ACME.mainThread, { item_id: 'accounted-live-answer', field: 'text', offset: 0, len: answer.length, data: answer });
    await expect(conversation.getByText(answer, { exact: true })).toBeVisible();
    const streamMs = Date.now() - sent;
    await emit(ACME.mainThread, {
      turns: [turn('accounted-live-turn', { ordinal: ROUNDS + 1, state: 'completed', inputId: 'accounted-live-input', startedAt: at, completedAt: at, tokens: 1_000 })],
      items: [agentMessage('accounted-live-answer', answer, { turn: 'accounted-live-turn', at })],
      thread: { state: 'idle', current_turn_id: null },
    });
    await expect(page.locator('[data-thread-header]').getByText('Idle', { exact: true })).toBeVisible();
    expect(reads.filter(isHistoryRead)).toHaveLength(syncsBeforeLive);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Performance.enable');
    const metrics = await cdp.send('Performance.getMetrics');
    const heap = metrics.metrics.find((metric) => metric.name === 'JSHeapUsedSize')?.value;
    await cdp.detach();
    const held = await page.evaluate(() => ({ rows: document.querySelectorAll('[role="log"] [data-index]').length, nodes: document.querySelectorAll('*').length }));
    expect(held.rows).toBeLessThan(ROWS);
    expect(held.nodes).toBeLessThan(NODES);
    expect(heap).toBeDefined();
    expect(heap).toBeLessThan(HEAP_BYTES);
    expect(errors).toEqual([]);
    await writeFile(info.outputPath('accounted-stress.json'), JSON.stringify({ rounds: ROUNDS, accountedTokens: TOKENS, openMs, streamMs, heapBytes: heap, ...held, snapshots, reads }, null, 2));
    await page.screenshot({ path: info.outputPath('accounted-live.png'), fullPage: true, animations: 'disabled' });
  });
}
