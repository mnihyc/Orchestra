/** 128 real browser Send interactions with native-shaped websocket history and a 10B-token lifetime counter.
 * This is a UI/transport bound, not a claim to generate ten billion tokens of model output. */
import { writeFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { ACME, configureThread, emit, scenario, signIn, threadHeld } from './support';
import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';

test('128 interaction rounds remain responsive with ten billion lifetime tokens and one websocket', async ({ page }, info) => {
  // 128 screen interactions plus actual HTTP/WS delivery need their own finite workflow budget.
  test.setTimeout(180_000);
  await scenario(); await configureThread(ACME.mainThread, { reset: true });
  await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null } });
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors: string[] = [], syncs: string[] = []; let sockets = 0, frames = 0, bytes = 0;
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('websocket', (socket) => { sockets += 1; socket.on('framereceived', ({ payload }) => { frames += 1; bytes += typeof payload === 'string' ? new TextEncoder().encode(payload).length : payload.byteLength; }); });
  page.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname === `/api/v1/threads/${ACME.mainThread}/sync`) syncs.push(request.url()); });
  await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  const main = page.locator('[data-screen="thread"]:visible');
  const composer = main.getByRole('textbox', { name: 'Message to the agent' });
  await expect(composer).toBeVisible();
  const connected = sockets, initialSyncs = syncs.length;
  const began = Date.now();
  for (let round = 1; round <= 128; round += 1) {
    const text = `Interaction ${round}: continue the long session.`;
    await composer.fill(text); await main.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(async () => (await threadHeld(ACME.mainThread)).inputs.length).toBe(round);
    const input = (await threadHeld(ACME.mainThread)).inputs.at(-1);
    if (input === undefined) throw new Error('The actual browser send has no retained input');
    const turnId = `long-turn-${round}`, at = new Date().toISOString();
    const completed = turn(turnId, { ordinal: round, state: 'completed', inputId: input.id, tokens: 78_125_000, startedAt: at, completedAt: at });
    // 128 × 78,125,000 = exactly 10,000,000,000 lifetime tokens; rendered bodies remain paged ordinary messages.
    await emit(ACME.mainThread, { turns: [completed], inputs: [{ ...threadInput(input.id, text, { author: ADA, position: String(round), state: 'delivered', clientId: input.client_id }), turn_id: turnId, delivered_at: at }],
      items: [userMessage(`long-user-${round}`, text, { author: ADA, inputId: input.id, turn: turnId, at }),
        agentMessage(`long-answer-${round}`, `Answer for interaction ${round}.\n\n${'A retained result with enough detail to exercise rendering and scrolling.\n\n'.repeat(32)}`, { turn: turnId, at })],
      thread: { state: 'idle', current_turn_id: null } });
    await expect(main.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
    await expect(composer).toHaveValue('');
  }
  await expect(main.getByText('Answer for interaction 128.', { exact: true })).toBeVisible();
  const held = await threadHeld(ACME.mainThread);
  expect(held.turns).toHaveLength(128);
  expect(held.turns.reduce((sum, turn) => sum + (turn.usage?.total_tokens ?? 0), 0)).toBe(10_000_000_000);
  expect(sockets).toBe(connected); expect(syncs).toHaveLength(initialSyncs); expect(errors).toEqual([]);
  const nodes = await main.locator('[data-item]').count();
  // Virtualized conversation rows must be fewer than the retained two-message-per-round history.
  expect(nodes).toBeLessThan(128);
  const log = main.getByRole('log', { name: 'Conversation', exact: true });
  await expect(log).not.toHaveAttribute('aria-busy', 'true'); await log.hover(); await page.mouse.wheel(0, -3000);
  await expect.poll(() => log.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeGreaterThan(1000);
  await composer.fill('The long session is still editable'); await expect(composer).toHaveValue('The long session is still editable');
  const metrics = { rounds: 128, lifetimeTokens: 10_000_000_000,
    elapsedMs: Date.now() - began, renderedItems: nodes, websocketConnectionsAfterOpening: sockets - connected, receivedFrames: frames, receivedBytes: bytes, additionalSyncReads: syncs.length - initialSyncs };
  await writeFile(info.outputPath('long-session-metrics.json'), JSON.stringify(metrics, null, 2));
  await info.attach('long-session-metrics.json', { contentType: 'application/json', path: info.outputPath('long-session-metrics.json') });
  await page.screenshot({ path: info.outputPath('long-session-128.png'), animations: 'disabled' });
});
