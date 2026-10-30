/** A conversation of five thousand items: how fast it opens, how many rows and nodes the page holds however long it is, how smoothly it scrolls from end to end and how soon a piece of text that arrives is on screen.
 *
 * What is measured, and how (a real browser, the development server, the fake API):
 * - Opening: in the page, from the start of the request for the thread to the first row drawn (`open`),
 *   split into the time the answer took (`answer`) and the time the page took to draw once it had it
 *   (`draw`). A MutationObserver notes the moment the first row appears. The thread is opened from
 *   another one by the router, after the page's own code has been loaded by the first: what is measured is
 *   the conversation, and not the development server handing its modules over one request at a time.
 * - What the page holds: the rows that are drawn in the conversation and all elements of the page, at
 *   the end of the conversation and after every older item has been read. A list that draws every item
 *   would hold some fifty thousand elements here; one that draws what is in view holds a few thousand.
 * - Scrolling: a script moves the conversation by 200 pixels a frame (about twelve thousand a second),
 *   down from the first item for 300 frames and back for 150, and the time between frames is taken from
 *   `requestAnimationFrame`; a long task is a task of the main thread of more than 50 ms (what the
 *   browser reports as one). The numbers that follow from them are the median and the 95th percentile
 *   of a frame, the longest frame, and the longest task. (Run by hand against the production build on the
 *   same busy machine, with a load average of 27 on 10 cores, the same scroll gave a median frame of
 *   17 ms, a 95th percentile of 33 ms and a longest of 67 ms; React's development build, which these
 *   tests run against, draws a row several times slower.)
 * - Memory: the JavaScript heap in use, after everything is held.
 * - Latency: from the moment the fake system sends a piece of text to the moment it stands in the page.
 *
 * The budgets are what a reader would not notice: the development build of React draws a row several
 * times slower than the production build does, so they are set to fail for a page that has become slow
 * and not for a machine that is. The budgets in milliseconds, and the time a test may take, are for a
 * quiet machine and are multiplied by how much slower than a quiet one this machine is just now (see
 * `slowdown`, and `FRAME_EXPONENT` for frames): a machine that has twice the work it has cores takes
 * twice the time for the same page.
 * The numbers measured are written into the report of the test; those that do not depend on how busy
 * the machine is (the rows and elements the page holds, the heap) are held tightly as they are.
 */
import { expect, type Page, test } from '@playwright/test';

import { ACME, delta, emit, playScene, scenario, signIn } from '../e2e/support.ts';
import { agentMessage, command, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';

const MAIN = ACME.mainThread;
const ITEMS = 5_000;
/** What a test may take, and how long it waits for a row to be drawn, on a quiet machine. */
const TEST_MS = 300_000;
const WAIT_MS = 60_000;
/** How long a reader at the top of the conversation may take to be handed every older item, a hundred at a time, on a quiet machine. */
const READ_ALL_MS = 120_000;
/** What a fixed piece of work (twenty million square roots) takes on a core with nothing else to do, on the machines these tests are written on. */
const QUIET_WORK_MS = 40;
/** A machine slower than this many times has nothing to measure: the factor stops here. */
const SLOWDOWN_AT_MOST = 12;
/**
 * Frames, and the long tasks that make them late, grow faster than the work does: the page and the browser's compositor each wait for the
 * processor in turn, and a frame that is late holds back the next. Measured on this machine the 95th percentile of a frame grew with about the
 * 1.5th power of the slowdown (633 ms at 2.3 times, 1 833 ms at 3.5, against some 250 on a quiet machine), so the budgets for frames and long
 * tasks are multiplied by that power, and the other budgets in milliseconds, and the time a test may take, by the slowdown itself.
 */
const FRAME_EXPONENT = 1.5;
/**
 * A measurement is worth taking when the machine is not overrun: the page's own limits (a jump to the end is held for four seconds at most)
 * are in seconds, which a machine many times slower than a quiet one does not leave it. A test waits, up to this long, for the machine to be
 * no more than `MEASURABLE_SLOWDOWN` times slower than a quiet one, looking every `QUIET_CHECK_MS`, and goes on with what it has if it
 * does not come to that.
 */
const WAIT_FOR_QUIET_MS = 600_000;
const MEASURABLE_SLOWDOWN = 2.5;
const QUIET_CHECK_MS = 20_000;
/** How far the conversation is moved each frame: a trackpad flung hard. */
const SCROLL_PIXELS = 200;

/** What a reader would not notice, in milliseconds or counts. */
const BUDGET = {
  /** From asking for the thread to its first rows: React's development build, on a machine that is busy with other work. */
  openMs: 8_000,
  /** From the answer to the first rows. */
  drawMs: 6_000,
  /** Rows drawn in the conversation at any moment, and all elements of the page. */
  rows: 90,
  nodes: 8_000,
  /** One frame in twenty may be slower than this while scrolling, and none slower than the longest. */
  frameP95Ms: 500,
  frameMaxMs: 2_500,
  longTaskMaxMs: 2_500,
  /** The JavaScript heap with all five thousand items held, in megabytes. */
  heapMb: 400,
  /** From a piece of text sent to it on screen. */
  streamMs: 3_000,
} as const;

test.beforeEach(async () => {
  await scenario();
  await emit(MAIN, { thread: { state: 'idle' } });
});

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });

/**
 * How many times slower than a machine with nothing else to do this one is just now, from the time a fixed piece of work takes in the page
 * (the median of five). Never less than one: a quiet machine is not given less than the budgets.
 */
async function slowdown(page: Page): Promise<number> {
  const took = await page.evaluate(() => {
    const times: number[] = [];
    for (let round = 0; round < 5; round += 1) {
      const start = performance.now();
      let sum = 0;
      for (let step = 0; step < 20_000_000; step += 1) sum += Math.sqrt(step);
      times.push(performance.now() - start + (sum < 0 ? 1 : 0));
    }
    times.sort((one, other) => one - other);
    return times[2] ?? 0;
  });
  return Math.min(SLOWDOWN_AT_MOST, Math.max(1, took / QUIET_WORK_MS));
}

/** The slowdown, once the machine is quiet enough to measure on (see `WAIT_FOR_QUIET_MS`), and the time the test is then given. */
async function whenMeasurable(page: Page, testStarted: number): Promise<number> {
  let slow = await slowdown(page);
  const until = Date.now() + WAIT_FOR_QUIET_MS;
  while (slow > MEASURABLE_SLOWDOWN && Date.now() < until) {
    await page.waitForTimeout(QUIET_CHECK_MS);
    slow = await slowdown(page);
  }
  test.setTimeout(Date.now() - testStarted + TEST_MS * slow);
  return slow;
}

interface Held {
  rows: number;
  nodes: number;
  heapMb: number | null;
}

/** What the page holds now. */
const held = (page: Page): Promise<Held> =>
  page.evaluate(() => {
    const memory = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
    return {
      rows: document.querySelectorAll('[role="log"] [data-index]').length,
      nodes: document.querySelectorAll('*').length,
      heapMb: memory === undefined ? null : Math.round(memory.usedJSHeapSize / 1_048_576),
    };
  });

interface Scrolled {
  frames: number;
  p50: number;
  p95: number;
  max: number;
  longTaskMax: number;
  longTasks: number;
  rowsMax: number;
}

/** Scrolls the conversation by `pixels` a frame for `steps` frames and says how the frames went. */
const scrollBy = (page: Page, pixels: number, steps: number): Promise<Scrolled> =>
  page.evaluate(
    ({ pixels, steps }) =>
      new Promise<Scrolled>((resolve) => {
        const log = document.querySelector<HTMLElement>('[role="log"]');
        if (log === null) throw new Error('no conversation');
        const frames: number[] = [];
        const tasks: number[] = [];
        let rowsMax = 0;
        const observer = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) tasks.push(entry.duration);
        });
        observer.observe({ entryTypes: ['longtask'] });
        let last = performance.now();
        let count = 0;
        const tick = (now: number) => {
          if (count > 0) frames.push(now - last);
          last = now;
          log.scrollTop += pixels;
          rowsMax = Math.max(rowsMax, log.querySelectorAll('[data-index]').length);
          count += 1;
          if (count <= steps) {
            requestAnimationFrame(tick);
            return;
          }
          observer.disconnect();
          const sorted = [...frames].sort((one, other) => one - other);
          const at = (share: number) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))] ?? 0;
          resolve({ frames: frames.length, p50: at(0.5), p95: at(0.95), max: sorted.at(-1) ?? 0, longTaskMax: Math.max(0, ...tasks), longTasks: tasks.length, rowsMax });
        };
        requestAnimationFrame(tick);
      }),
    { pixels, steps },
  );

function report(name: string, numbers: Record<string, number | null>): void {
  const text = Object.entries(numbers)
    .map(([key, value]) => `${key}=${value === null ? 'n/a' : Math.round(value * 10) / 10}`)
    .join(' ');
  test.info().annotations.push({ type: name, description: text });
  console.warn(`[measured] ${name}: ${text}`);
}

test('a conversation of five thousand items opens at its end in moments, and holds only what is in view', async ({ page }) => {
  const testStarted = Date.now();
  test.setTimeout(TEST_MS + WAIT_FOR_QUIET_MS);
  await playScene(MAIN, 'long', ITEMS);
  await playScene(ACME.depsThread, 'long', ITEMS);
  await page.addInitScript(() => {
    // The development server hands over hundreds of modules: the default room for entries would be full.
    performance.setResourceTimingBufferSize(10_000);
    const note = { firstRow: null as number | null, previous: null as Element | null, clicked: 0 };
    (window as unknown as { __open: typeof note }).__open = note;
    new MutationObserver(() => {
      // The first row of a conversation other than the one that was open when the link was pressed.
      const log = document.querySelector('[role="log"]');
      if (note.firstRow === null && log !== null && log !== note.previous && log.querySelector('[data-index]') !== null) note.firstRow = performance.now();
    }).observe(document, { childList: true, subtree: true });
  });
  await signIn(page);
  let slow = await whenMeasurable(page, testStarted);
  // Warm: the first thread loads the page's code; the second is then opened by the router.
  await page.goto(`/thread/${MAIN}`);
  await expect(conversation(page).getByText(/Question 8\d\d:/).first()).toBeVisible({ timeout: WAIT_MS * slow });
  await page.getByRole('tree', { name: 'Orgs, projects, topics and threads' }).getByRole('treeitem', { name: /Upgrade deps/ }).click();
  const threads = page.getByTestId('main').getByRole('link', { name: 'Main thread' });
  await expect(threads.first()).toBeVisible();
  await page.evaluate(() => {
    const note = (window as unknown as { __open: { firstRow: number | null; previous: Element | null; clicked: number } }).__open;
    note.firstRow = null;
    note.previous = document.querySelector('[role="log"]');
    note.clicked = performance.now();
  });
  await threads.first().click();
  await expect(conversation(page).getByText(/Question 8\d\d:/).first()).toBeVisible({ timeout: WAIT_MS * slow });

  const times = await page.evaluate((thread) => {
    const note = (window as unknown as { __open: { firstRow: number | null; clicked: number } }).__open;
    const sync = performance.getEntriesByType('resource').filter((entry) => entry.name.includes(`/threads/${thread}/sync`)).at(-1) as PerformanceResourceTiming | undefined;
    return { first: note.firstRow, clicked: note.clicked, start: sync?.startTime ?? null, end: sync?.responseEnd ?? null };
  }, ACME.depsThread);
  expect(times.first).not.toBeNull();
  expect(times.start).not.toBeNull();
  const open = (times.first ?? 0) - (times.start ?? 0);
  const answer = (times.end ?? 0) - (times.start ?? 0);
  const draw = (times.first ?? 0) - (times.end ?? 0);
  const fromClick = (times.first ?? 0) - times.clicked;
  const atEnd = await held(page);
  slow = Math.max(slow, await slowdown(page));
  report('open', { fromClick, open, answer, draw, rows: atEnd.rows, nodes: atEnd.nodes, slowdown: slow });
  expect(open, 'from asking for the thread to its first rows').toBeLessThan(BUDGET.openMs * slow);
  expect(draw, 'from the answer to its first rows').toBeLessThan(BUDGET.drawMs * slow);
  expect(atEnd.rows).toBeLessThan(BUDGET.rows);
  expect(atEnd.nodes).toBeLessThan(BUDGET.nodes);

  // It is at its end: the last question and its answer are the ones in view.
  await expect(conversation(page).getByText(/Question 834:/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Jump to latest' })).toHaveCount(0);

  // Every older item is read, a hundred at a time, as the reader comes to the top.
  const reachedTop = async (): Promise<boolean> => {
    await page.evaluate(() => {
      const log = document.querySelector<HTMLElement>('[role="log"]');
      if (log !== null) log.scrollTop = 0;
    });
    await page.waitForTimeout(120);
    return page.evaluate(() => (document.querySelector('[role="log"]')?.textContent ?? '').includes('Question 1:'));
  };
  const opened = Date.now();
  while (Date.now() - opened < READ_ALL_MS * slow && !(await reachedTop()));
  expect(await reachedTop(), 'the first item was reached').toBe(true);
  const readAll = Date.now() - opened;
  const all = await held(page);
  report('all items held', { readMs: readAll, rows: all.rows, nodes: all.nodes, heapMb: all.heapMb });
  expect(all.rows).toBeLessThan(BUDGET.rows);
  expect(all.nodes).toBeLessThan(BUDGET.nodes);
  if (all.heapMb !== null) expect(all.heapMb).toBeLessThan(BUDGET.heapMb);

  // From the first item down, 200 pixels a frame (about 12 000 a second: a trackpad flung hard), for 300 frames.
  const scrolled = await scrollBy(page, SCROLL_PIXELS, 300);
  slow = Math.max(slow, await slowdown(page));
  report('scroll down', { frames: scrolled.frames, p50: scrolled.p50, p95: scrolled.p95, max: scrolled.max, longTasks: scrolled.longTasks, longTaskMax: scrolled.longTaskMax, rowsMax: scrolled.rowsMax, slowdown: slow });
  const late = slow ** FRAME_EXPONENT;
  expect(scrolled.p95, 'a frame, at the 95th percentile').toBeLessThan(BUDGET.frameP95Ms * late);
  expect(scrolled.max, 'the longest frame').toBeLessThan(BUDGET.frameMaxMs * late);
  expect(scrolled.longTaskMax, 'the longest task').toBeLessThan(BUDGET.longTaskMaxMs * late);
  expect(scrolled.rowsMax).toBeLessThan(BUDGET.rows);

  // And back up.
  const back = await scrollBy(page, -SCROLL_PIXELS, 150);
  slow = Math.max(slow, await slowdown(page));
  report('scroll up', { frames: back.frames, p50: back.p50, p95: back.p95, max: back.max, longTasks: back.longTasks, longTaskMax: back.longTaskMax, rowsMax: back.rowsMax, slowdown: slow });
  expect(back.p95).toBeLessThan(BUDGET.frameP95Ms * slow ** FRAME_EXPONENT);
  expect(back.rowsMax).toBeLessThan(BUDGET.rows);
});

test('text that arrives in a conversation of five thousand items is on screen at once, and the reader at the end stays there', async ({ page }) => {
  const testStarted = Date.now();
  test.setTimeout(TEST_MS + WAIT_FOR_QUIET_MS);
  await playScene(MAIN, 'long', ITEMS);
  await signIn(page);
  const slow = await whenMeasurable(page, testStarted);
  await page.goto(`/thread/${MAIN}`);
  await expect(conversation(page).getByText(/Question 834:/)).toBeVisible({ timeout: WAIT_MS * slow });

  // A turn begins: the text of its answer grows in pieces.
  const began = new Date().toISOString();
  await emit(MAIN, {
    items: [
      userMessage('live-u', 'Question 835: and now?', { author: ADA, turn: 'live-t', inputId: 'live-in', at: began }),
      agentMessage('live-a', '', { turn: 'live-t', status: 'in_progress', at: began }),
    ],
    turns: [turn('live-t', { ordinal: 835, state: 'running', inputId: 'live-in', startedAt: began, phase: { kind: 'working', since: began } })],
    thread: { state: 'running' },
  });
  await expect(conversation(page).getByText('Question 835: and now?')).toBeVisible({ timeout: WAIT_MS * slow });

  const word = 'streaming';
  const sentAt = Date.now();
  await delta(MAIN, { item_id: 'live-a', field: 'text', offset: 0, len: word.length, data: word });
  await expect(conversation(page).getByText(word)).toBeVisible({ timeout: WAIT_MS * slow });
  const streamed = Date.now() - sentAt;
  report('stream', { toScreenMs: streamed, slowdown: slow });
  expect(streamed).toBeLessThan(BUDGET.streamMs * slow);

  // Many more pieces, and a command with long output: the reader stays at the end the whole time.
  let offset = word.length;
  for (let n = 0; n < 60; n += 1) {
    const piece = ` piece${n}`;
    await delta(MAIN, { item_id: 'live-a', field: 'text', offset, len: piece.length, data: piece });
    offset += piece.length;
  }
  await emit(MAIN, { items: [command('live-c', { command: 'npm test', output: Array.from({ length: 200 }, (_, line) => `output line ${line}`).join('\n'), exit: 0, turn: 'live-t', at: began })] });
  await expect(conversation(page).getByText('piece59')).toBeVisible({ timeout: WAIT_MS * slow });
  await expect(page.getByRole('button', { name: 'Jump to latest' })).toHaveCount(0);
  const now = await held(page);
  expect(now.rows).toBeLessThan(BUDGET.rows);
});

test('a link to an item near the beginning of a conversation of five thousand items opens a window around it', async ({ page }) => {
  const testStarted = Date.now();
  test.setTimeout(TEST_MS + WAIT_FOR_QUIET_MS);
  await playScene(MAIN, 'long', ITEMS);
  await signIn(page);
  const slow = await whenMeasurable(page, testStarted);
  const started = Date.now();
  await page.goto(`/thread/${MAIN}?item=lu12`);
  await expect(conversation(page).getByText(/Question 12:/)).toBeVisible({ timeout: WAIT_MS * slow });
  report('permalink', { toScreenMs: Date.now() - started });
  const now = await held(page);
  expect(now.rows).toBeLessThan(BUDGET.rows);
  // The newest items are not held, so the way back to them is offered.
  await expect(page.getByRole('button', { name: 'Jump to latest' })).toBeVisible();
});
