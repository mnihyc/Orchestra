/** Stop, whichever of its answer and the frame that moves the thread comes first, or neither for a while: after the press the screen never
 * offers Stop again for that turn, shows that stopping is under way until the turn is over, shows the turn ended when the server says so by
 * either path (and the other, coming later, does not undo it), and a failure to stop is told with the act offered again. The answer, the
 * frames and the connection are held at a chosen moment, not left to the order they happen to come in. */
import { expect, test, type Page, type Route, type WebSocketRoute } from '@playwright/test';

import { ADA } from '../doubles/thread-scenes.ts';
import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { ACME, dropConnections, emit, PEOPLE, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;
const INTERRUPT = /\/threads\/[^/]+\/interrupt$/;

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });
/** The Stop button says "Stop", and "Stopping…" (with the time once it lasts more than a second) while the stop is under way. */
const stop = (page: Page) => page.getByRole('group', { name: 'Message controls', exact: true }).getByRole('button', { name: /^Stop(?:ping….*)?$/u });
const stopped = (page: Page) => conversation(page).getByText('Stopped by Ada Lovelace');
/** The thread's tab in the main area says what the thread's own state is. */
const working = (page: Page) => page.getByRole('tab', { name: /^Running Main thread\b/u });

/** Something to wait on that the test opens when it chooses. */
function gate() {
  let open!: () => void;
  const opened = new Promise<void>((resolve) => { open = resolve; });
  return { opened, open };
}

/** The thread open, a turn running, and the page quiet. */
async function running(page: Page, width: number): Promise<void> {
  await page.setViewportSize({ width, height: 900 });
  await scenario();
  await emit(MAIN, { thread: { state: 'idle' } });
}

async function turnBegins(page: Page): Promise<void> {
  const began = new Date().toISOString();
  await emit(MAIN, {
    items: [userMessage('u1', 'Fix the login redirect', { author: ADA, turn: 't1', inputId: 'in-1', at: began }), agentMessage('a1', '', { turn: 't1', status: 'in_progress', at: began })],
    turns: [turn('t1', { ordinal: 1, state: 'running', inputId: 'in-1', startedAt: began, phase: { kind: 'working', since: began } })],
    inputs: [{ ...threadInput('in-1', 'Fix the login redirect', { author: ADA, position: '1', state: 'delivered' }), turn_id: 't1', delivered_at: began }],
    thread: { state: 'running' },
  });
  await expect(stop(page)).toBeVisible();
  await expect(working(page)).toBeVisible();
}

async function open(page: Page, width: number): Promise<void> {
  await running(page, width);
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${MAIN}`);
  await turnBegins(page);
}

/** What the page is sent over the socket is held from `hold()` to `release()`; what it sends is not. */
async function frames(page: Page) {
  const held: (() => void)[] = [];
  let holding = false;
  await page.routeWebSocket(/\/ws$/, (route: WebSocketRoute) => {
    const server = route.connectToServer();
    server.onMessage((message) => { if (holding) held.push(() => route.send(message)); else route.send(message); });
    route.onMessage((message) => server.send(message));
  });
  return {
    hold: () => { holding = true; },
    release: () => { holding = false; for (const send of held.splice(0)) send(); },
  };
}

/** The stop requests the page makes, counted. */
function sent(page: Page): () => number {
  let count = 0;
  page.on('request', (request) => { if (request.method() === 'POST' && INTERRUPT.test(new URL(request.url()).pathname)) count += 1; });
  return () => count;
}

for (const width of [1440, 400]) {
  test.describe(`${width}`, () => {
    test(`the answer to Stop says the turn is over before the frame that moves the thread comes: Stop is not offered again, and the frame then ends the rest (${width})`, async ({ page }) => {
      const socket = await frames(page);
      const stops = sent(page);
      await open(page, width);
      socket.hold();
      await stop(page).click();
      // The answer is in: the turn shows as stopped. The thread's own state has not moved, and the turn it names is over.
      await expect(stopped(page)).toBeVisible();
      await expect(stop(page), 'a turn that is over is not stopped again').toHaveCount(0, { timeout: 5_000 });
      expect(stops()).toBe(1);
      socket.release();
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    test(`the frame that ends the turn comes before the answer to Stop, and the answer, late, does not undo it (${width})`, async ({ page }) => {
      const answer = gate();
      await page.route(INTERRUPT, async (route: Route) => { const response = await route.fetch(); await answer.opened; await route.fulfill({ response }); });
      await open(page, width);
      await stop(page).click();
      await expect(stopped(page)).toBeVisible();
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
      answer.open();
      await page.waitForTimeout(1_500);
      await expect(stopped(page)).toBeVisible();
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    test(`while the request is held, stopping is shown as under way and is not asked twice; when it is let through the turn ends (${width})`, async ({ page }) => {
      const request = gate();
      await page.route(INTERRUPT, async (route: Route) => { await request.opened; await route.continue(); });
      const stops = sent(page);
      await open(page, width);
      await stop(page).click();
      await expect(stop(page)).toHaveAttribute('aria-busy', 'true');
      await expect(stop(page)).toHaveAttribute('aria-disabled', 'true');
      // A second press, and the key for it, ask nothing more.
      await stop(page).click({ force: true });
      await page.keyboard.press('Escape');
      await page.keyboard.press('Escape');
      await page.waitForTimeout(800);
      expect(stops()).toBe(1);
      await expect(stopped(page)).toHaveCount(0);
      request.open();
      await expect(stopped(page)).toBeVisible();
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
    });

    test(`the connection dropped between the press and the answer leaves stopping under way, and the turn ends when the answer comes (${width})`, async ({ page }) => {
      const request = gate();
      await page.route(INTERRUPT, async (route: Route) => { await request.opened; await route.continue(); });
      await open(page, width);
      await stop(page).click();
      const baseline = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/me/bootstrap', { timeout: 30_000 });
      await dropConnections();
      await baseline;
      await page.waitForTimeout(1_000);
      await expect(stop(page)).toHaveAttribute('aria-busy', 'true');
      await expect(stopped(page)).toHaveCount(0);
      request.open();
      await expect(stopped(page)).toBeVisible();
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
    });

    test(`the answer held while the connection is dropped and found again: the turn stays ended (${width})`, async ({ page }) => {
      const answer = gate();
      await page.route(INTERRUPT, async (route: Route) => { const response = await route.fetch(); await answer.opened; await route.fulfill({ response }); });
      await open(page, width);
      await stop(page).click();
      await expect(stopped(page)).toBeVisible();
      const baseline = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/me/bootstrap', { timeout: 30_000 });
      await dropConnections();
      await baseline;
      await page.waitForTimeout(1_000);
      await expect(working(page)).toHaveCount(0);
      await expect(stop(page)).toHaveCount(0);
      answer.open();
      await page.waitForTimeout(1_000);
      await expect(stopped(page)).toBeVisible();
      await expect(stop(page)).toHaveCount(0);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    for (const [failure, words] of [['the server fails', 'The server could not complete this request. Try again.'], ['the network is lost', 'The server could not be reached. Try again.']] as const) {
      test(`a Stop that fails because ${failure} is told, and Stop is offered again and works (${width})`, async ({ page }) => {
        let failing = true;
        await page.route(INTERRUPT, async (route: Route) => {
          if (!failing) { await route.continue(); return; }
          if (failure === 'the network is lost') { await route.abort('connectionreset'); return; }
          await route.fulfill({ status: 500, contentType: 'application/problem+json', body: JSON.stringify({ type: 'orch:error:internal', title: 'Failed', status: 500, code: 'internal', detail: 'The server failed.', request_id: 'r-stop' }) });
        });
        await open(page, width);
        await stop(page).click();
        await expect(page.getByRole('alert').filter({ hasText: words })).toBeVisible();
        await expect(stop(page)).toBeVisible();
        await expect(stop(page)).not.toHaveAttribute('aria-disabled', 'true');
        failing = false;
        await stop(page).click();
        await expect(stopped(page)).toBeVisible();
        await expect(working(page)).toHaveCount(0);
        await expect(stop(page)).toHaveCount(0);
      });
    }

    test(`Stop after the page has marked the thread read leaves it idle everywhere: the state it had while read is not kept (${width})`, async ({ page }) => {
      let reads = 0;
      page.on('request', (request) => { if (request.method() === 'POST' && /\/threads\/[^/]+\/read$/.test(new URL(request.url()).pathname)) reads += 1; });
      await open(page, width);
      // The newest item is marked read after it has been on screen two seconds; that read is answered while the turn runs.
      await expect.poll(() => reads, { timeout: 15_000 }).toBeGreaterThan(0);
      await page.waitForTimeout(500);
      await stop(page).click();
      await expect(stopped(page)).toBeVisible();
      await expect(working(page)).toHaveCount(0, { timeout: 10_000 });
      await expect(page.locator('[data-thread-header] [data-state="running"]')).toHaveCount(0);
    });
  });
}
