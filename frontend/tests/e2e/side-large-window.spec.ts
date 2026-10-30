/** Production-browser cost of the real side renderer; synthetic HTTP is not native agent proof. */
import { expect, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import type { ThreadOpening, ThreadSync } from '../../src/generated/api-types';
import { agentMessage } from '../doubles/thread-rows';
import { conversation, openHistory } from './change-support';
import { scenario } from './support';

test('a full 200-item side window remains usable inline and in its own panel', async ({ page }, info) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  const openings: string[] = [], writes: string[] = [], errors: string[] = [];
  let acceptedSide: string | null = null;
  let sideSequence = 0;
  const frames: object[] = [];
  page.on('websocket', (socket) => {
    const record = (direction: string, payload: string | Buffer) => {
      const value = JSON.parse(String(payload)) as { op?: string; id?: number; feed?: string; seq?: number; prev?: number;
        data?: { projection?: unknown; items?: unknown[]; thread?: { sides?: { open: number } } } };
      if (value.feed?.startsWith('thread:') === true) frames.push({ direction, op: value.op, id: value.id, feed: value.feed,
        seq: value.seq, prev: value.prev, projection: value.data?.projection, items: value.data?.items?.length,
        sideCount: value.data?.thread?.sides?.open });
    };
    socket.on('framesent', ({ payload }) => record('sent', payload));
    socket.on('framereceived', ({ payload }) => record('received', payload));
  });
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== 'GET' && /\/threads\/[^/]+\/(?:side|inputs|interrupt|continuations|binding)(?:\/|$)/.test(path)) writes.push(path);
  });
  // Keep the scripted 200-item snapshot and its subscription at the same cut.
  // Other feeds use the ordinary server through the same physical socket.
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (socket) => {
    const server = socket.connectToServer(), sideSubscriptions = new Set<number>();
    socket.onMessage((raw) => {
      const frame = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
      if (acceptedSide !== null && frame.op === 'sub' && frame.feed === `thread:${acceptedSide}` && frame.id !== undefined) {
        sideSubscriptions.add(frame.id);
        socket.send(JSON.stringify({ op: 'subscribed', id: frame.id, feed: frame.feed, mode: 'delta', seq: sideSequence }));
      } else if (frame.op === 'unsub' && frame.id !== undefined && sideSubscriptions.delete(frame.id)) {
        socket.send(JSON.stringify({ op: 'closed', id: frame.id, reason: 'unsubscribed' }));
      } else server.send(raw);
    });
    socket.onClose((code, reason) => { void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
  });
  await page.route('**/api/v1/threads/*/side', async (route) => {
    if (route.request().method() !== 'POST') { await route.continue(); return; }
    const response = await route.fetch();
    expect(response.status()).toBe(201);
    const reply = await response.json() as { thread: { id: string } };
    acceptedSide = reply.thread.id;
    await route.fulfill({ response });
  });
  await page.route('**/api/v1/threads/*/opening?*', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.split('/')[4] !== acceptedSide) { await route.continue(); return; }
    // The legacy side double exposes its own authorized sync, but lacks the
    // aggregate's supplementary readers. This renderer probe supplies only
    // nullable/empty supplements; it never borrows the parent's opening data.
    url.pathname = url.pathname.replace(/\/opening$/, '/sync');
    const response = await route.fetch({ url: url.toString() });
    expect(response.ok()).toBe(true);
    const sync = await response.json() as ThreadSync;
    const data: ThreadOpening = { sync, draft: null, parameters: null, limits: null, settings: [],
      outline: { entries: [], next_before: null, seq: sync.seq } };
    const id = data.sync.thread.id;
    openings.push(id);
    data.sync.items = Array.from({ length: 200 }, (_, index) => ({
      ...agentMessage(`side-window-${index}`, `Retained side answer ${index}`),
      thread_id: id, continuation_id: data.sync.projection?.viewed_continuation_id ?? null,
      seq_created: index + 1, seq_updated: index + 1,
    }));
    data.sync.prompt_summaries = [];
    sideSequence = sync.seq + data.sync.items.length;
    data.sync.seq = sideSequence;
    data.sync.thread = { ...sync.thread, seq: sideSequence, state: 'idle', current_turn_id: null };
    data.sync.turns = [];
    data.sync.has_older = true;
    await route.fulfill({ json: data });
  });
  await openHistory(page);
  const draft = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
  await draft.fill('Keep the parent draft during the large side read');
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Ask on the side…', exact: true }).click();
  const ask = page.getByRole('dialog', { name: 'Ask on the side', exact: true });
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('Review the retained side history');
  const inlineStart = await page.evaluate(() => performance.now());
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();
  const lane = conversation(page).locator('[data-side-lane]');
  try { await expect(lane.locator('[data-item]')).toHaveCount(200); }
  catch (error) { console.warn('side-opening-diagnostic', JSON.stringify({ openings, writes, errors, frames })); throw error; }
  await expect(lane.getByText('Retained side answer 199', { exact: true })).toBeAttached();
  const inlineMs = await page.evaluate((start) => performance.now() - start, inlineStart);
  await expect(draft).toHaveValue('Keep the parent draft during the large side read');
  const panelStart = await page.evaluate(() => performance.now());
  await lane.getByRole('button', { name: 'Open in panel', exact: true }).click();
  const panel = page.locator('[data-side-panel]');
  await expect(panel.locator('[data-item]')).toHaveCount(200);
  const last = panel.getByText('Retained side answer 199', { exact: true });
  await last.scrollIntoViewIfNeeded();
  await expect(last).toBeVisible();
  const panelMs = await page.evaluate((start) => performance.now() - start, panelStart);
  await panel.getByRole('textbox').fill('A separate side draft');
  await expect(draft).toHaveValue('Keep the parent draft during the large side read');
  expect(openings).toHaveLength(2);
  expect(new Set(openings).size).toBe(1);
  expect(writes).toHaveLength(1);
  expect(writes[0]).toMatch(/\/side$/);
  expect(errors).toEqual([]);
  const metrics = { inlineMs, panelMs, items: 200, openings: openings.length,
    nodes: await panel.locator('*').count(), scope: 'Local production browser with native-shaped synthetic HTTP; no vendor/model claim' };
  await writeFile(info.outputPath('side-window-metrics.json'), JSON.stringify(metrics, null, 2));
  await info.attach('side-window-metrics', { path: info.outputPath('side-window-metrics.json'), contentType: 'application/json' });
  await page.screenshot({ path: info.outputPath('large-side-panel.png'), animations: 'disabled' });
});
