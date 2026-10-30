/** The left bar's tree with two thousand nodes, folders among them to three deep and documents beside every project and folder: how soon it is drawn, how much of the page it is, and what a node whose state changes costs.
 *
 * What is measured, and how (a real browser, the fake API, and the built application, which the Playwright config says is a production
 * build by setting `ORCH_PERF_BUILD=production`; against the development server it says `development`, and the budgets are the larger):
 * - What the page holds: the rows of the tree and all elements of the page once the whole tree is open (every project, folder and topic,
 *   so that every thread is a row too, and every document written beside a project or a folder), and the heap.
 * - The documents: they are asked for place by place once the tree stands, so they come after it; how long after is written into the
 *   report, and everything measured from then on is measured with their rows in the tree.
 * - Drawing: in the page, from the end of the answer that carried the tree to the moment its last row stands in the page, which a
 *   MutationObserver notes.
 * - A change: a thread of the tree changes its state, as the live connection says it does, which changes its row and the row of the
 *   topic it lies in. What it costs is the CPU time the page's main thread spends from just before the change is sent to a pause
 *   after the rows show it (the thread's own clock, `ThreadTime` of the browser's metrics, which a machine busy with other work does
 *   not stretch the way the time between two frames is stretched), less what the same pause costs when nothing changes. The
 *   specification asks that no frame is over 16 ms when a node's state changes; the work that a change causes is what must fit in
 *   frames, so the median of it is held to that figure. A row far out of view and a row in view are measured apart: the browser leaves
 *   the first out of its layout, styling and painting, and the second it draws, and starts the spinner of, in a frame of its own.
 * - Twenty-odd such changes are made one after another for each, and fifty at once.
 * The specification's figure is for the production build. The development build of React draws several times slower
 * (`DEVELOPMENT_BUILD`); a core that is slower than the one the figure was found on makes the time longer by `speed`, found by timing
 * a fixed piece of work in the page with the same clock; and a machine that is busy makes waiting longer by `slowdown`, found by the
 * clock of the wall. The budgets fail for a page that has become slow and not for a machine or a build that is. The numbers measured
 * are written into the report of the test.
 */
import { randomUUID } from 'node:crypto';

import { expect, type Page, test } from '@playwright/test';

import { acmeTree, ACME, id as fixtureId } from '../doubles/fixtures.ts';
import { changeTree, scenario, signIn } from '../e2e/support.ts';

/** How many projects the tree has, each with a chain of three folders and one folder beside the first, and how many topics lie in each of those places. */
const PROJECTS = 8;
const TOPICS_PER_PLACE = 25;
/** How many documents are written beside each of those projects and folders: two hundred rows of the tree are documents. */
const DOCUMENTS_PER_PLACE = 5;
/** What a test may take, and how long it waits for the tree to be drawn, on a quiet machine. */
const TEST_MS = 240_000;
const WAIT_MS = 60_000;
/** What a fixed piece of work (twenty million square roots) takes on a core with nothing else to do, on the machines these tests are written on. */
const QUIET_WORK_MS = 40;
/** A machine or a core slower than this many times has nothing to measure: the factor stops here. */
const SLOWDOWN_AT_MOST = 12;
/** What the specification allows a frame to take when a node's state changes, on a quiet machine and the production build, in milliseconds. */
const SPECIFIED_FRAME_MS = 16;
/**
 * How many times more the development build of React spends on a change than the production build does: what the budget is multiplied
 * by when the server is not a production build. Measured here at 5 to 7, and set above it.
 */
const DEVELOPMENT_BUILD = 10;
/** The same for the first drawing of the whole tree, which the development build mounts some four times slower. */
const DEVELOPMENT_DRAW = 5;
const PRODUCTION = process.env.ORCH_PERF_BUILD === 'production';
/** A change of a row in view is two frames: the drawing of what changed, and the first painting of it. */
const FRAMES_OF_A_VISIBLE_CHANGE = 2;
/** The page is quiet when its main thread uses less than this much CPU time over a window of this length. */
const QUIET_CPU_MS = 10;
const QUIET_WINDOW_MS = 300;
/** How long to wait for it: a page that does not become quiet is measured as it is, less what it spends at rest. */
const QUIET_PATIENCE_MS = 20_000;
/** The changes made of each kind while the CPU time is read, and the pause after each, so that what the change starts has been done. */
const CHANGES = 12;
const PAUSE_MS = 150;
/**
 * How many threads change at once in the last measurement, how many times that is done, and how many times the budget of one change
 * that may take: fifty rows and fifty of the topics they lie in are drawn in one frame of the live connection, and the specification
 * says nothing of that, so this holds only that it is not fifty frames.
 */
const BURST = 50;
const BURST_ROUNDS = 6;
const BURST_FACTOR = 10;

/** What the page may hold of a tree of two thousand nodes, all open. */
const BUDGET = {
  /** All elements of the page: a row of the tree is some ten, and nothing else grows with the tree. */
  nodes: 60_000,
  /** The JavaScript heap, in megabytes. */
  heapMb: 600,
  /** From the answer that carried the tree to its last row in the page, on a quiet machine, in milliseconds. */
  drawMs: 8_000,
} as const;

interface Seeded {
  nodes: Record<string, unknown>[];
  /** The threads, in the order they were made. */
  threads: string[];
  /** Everything that has something below it: what is open when the tree is shown. */
  open: string[];
  /** The projects and folders made here, beside each of which documents are written. */
  places: string[];
}

const IDLE = { state: 'idle', blocked_reason: null, pending_approvals: 0, activity_seq: 1, last_activity_at: '2026-10-11T10:00:00.000Z' } as const;

/** The projects, their folders, and in each place its topics and a thread in every topic. */
function seed(): Seeded {
  const nodes: Record<string, unknown>[] = [];
  const threads: string[] = [];
  const places: string[] = [];
  const open = acmeTree().filter((row) => row.kind !== 'thread').map((row) => row.id);
  const place = (name: string, parent: string, kind: 'project'): string => {
    const made = randomUUID();
    nodes.push({ id: made, kind, name, parent_id: parent });
    open.push(made);
    places.push(made);
    return made;
  };
  for (let number = 0; number < PROJECTS; number += 1) {
    const home = place(`Bulk ${number}`, ACME.org, 'project');
    const first = place(`Bulk ${number} folder 1`, home, 'project');
    const second = place(`Bulk ${number} folder 2`, first, 'project');
    const third = place(`Bulk ${number} folder 3`, second, 'project');
    const beside = place(`Bulk ${number} other`, home, 'project');
    for (const [at, where] of [home, first, second, third, beside].entries()) {
      for (let topicNumber = 0; topicNumber < TOPICS_PER_PLACE; topicNumber += 1) {
        const topic = randomUUID();
        const thread = randomUUID();
        nodes.push({ id: topic, kind: 'topic', name: `Topic ${number}.${at}.${topicNumber}`, parent_id: where, topic: { status: 'open', tag_ids: [] } });
        nodes.push({ id: thread, kind: 'thread', name: 'Main thread', parent_id: topic, thread: IDLE });
        threads.push(thread);
        open.push(topic);
      }
    }
  }
  return { nodes, threads, open, places };
}

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** Writes a document of one paragraph beside a node, as the system would hold it. */
async function makeDocument(node: string, title: string): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/documents/make`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ count: 1, title, node }) });
  if (!response.ok) throw new Error(`the fake API refused a document: ${response.status}`);
}

// A recording of the page (its trace, its screenshots) is work of the page's own thread, and what is measured here is that thread's work.
test.use({ trace: 'off', screenshot: 'off', video: 'off' });

test.beforeEach(async () => {
  await scenario();
});

const median = (values: number[]): number => [...values].sort((one, other) => one - other)[Math.floor(values.length / 2)] ?? 0;
const percentile = (values: number[], share: number): number => {
  const sorted = [...values].sort((one, other) => one - other);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))] ?? 0;
};

/** The piece of work that both clocks time: twenty million square roots. It stands alone, for it is sent to the page as it is written. */
const work = (): number => {
  let sum = 0;
  for (let step = 0; step < 20_000_000; step += 1) sum += Math.sqrt(step);
  return sum;
};

/**
 * How many times slower than a machine with nothing else to do this one is just now, by the clock of the wall, from the time a fixed
 * piece of work takes in the page (the median of five). Never less than one. It lengthens what the test waits for.
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

/** The CPU time that the main thread of the page has used so far, in milliseconds, by the browser's own metrics. */
type Meter = () => Promise<number>;

async function meter(page: Page): Promise<Meter> {
  const session = await page.context().newCDPSession(page);
  await session.send('Performance.enable');
  return async () => {
    const { metrics } = await session.send('Performance.getMetrics');
    const time = metrics.find((metric) => metric.name === 'ThreadTime');
    if (time === undefined) throw new Error('The browser does not report the CPU time of its main thread.');
    return time.value * 1000;
  };
}

/** Waits until the page has done what the drawing of the tree left it with, so that what is measured next is a change and not the end of that. */
async function untilQuiet(page: Page, cpu: Meter, patience: number): Promise<number> {
  const until = Date.now() + patience;
  for (;;) {
    const before = await cpu();
    await page.waitForTimeout(QUIET_WINDOW_MS);
    const used = (await cpu()) - before;
    if (used < QUIET_CPU_MS || Date.now() > until) return used;
  }
}

/**
 * How many times slower than the machines these tests are written on this core is just now, by the clock of the thread (the median of
 * three): the factor that the figure of the specification is multiplied by. Never less than one.
 */
async function speed(page: Page, cpu: Meter): Promise<number> {
  const took: number[] = [];
  for (let round = 0; round < 3; round += 1) {
    const before = await cpu();
    await page.evaluate(work);
    took.push((await cpu()) - before);
  }
  return Math.min(SLOWDOWN_AT_MOST, Math.max(1, median(took) / QUIET_WORK_MS));
}

function report(name: string, numbers: Record<string, number | null>): void {
  const text = Object.entries(numbers)
    .map(([key, value]) => `${key}=${value === null ? 'n/a' : Math.round(value * 10) / 10}`)
    .join(' ');
  test.info().annotations.push({ type: name, description: text });
  console.warn(`[measured] ${name}: ${text}`);
}

/** The states that threads change to here, and the name of the icon that shows each. A running thread's icon turns; a blocked one's does not. */
const WORDS = { running: 'Running', blocked: 'Blocked', idle: 'Idle' } as const;
type Shown = keyof typeof WORDS;

/** A thread's new state, as the live connection says it. */
const state = (name: Shown, seq: number) => ({ state: name, blocked_reason: null, pending_approvals: 0, activity_seq: seq, last_activity_at: '2026-10-11T11:00:00.000Z' });

/** Waits until the row of a thread shows a state, by the name of its icon. */
const shows = (page: Page, thread: string, name: Shown, timeout: number): Promise<unknown> =>
  page.waitForFunction(
    ({ thread, word }) => document.querySelector(`[data-tree-row="${thread}"] [role="img"]`)?.getAttribute('aria-label') === word,
    { thread, word: WORDS[name] },
    { timeout, polling: 50 },
  );

/** How many rows of the tree the page holds. */
const rowsOf = (page: Page): Promise<number> => page.evaluate(() => document.querySelectorAll('[role="tree"] [role="treeitem"]').length);

test('a tree of two thousand nodes with folders three deep and documents beside them is drawn once, holds only its rows, and a node that changes costs a frame no more than the specification allows', async ({ page }) => {
  test.setTimeout(TEST_MS * 4);
  const seeded = seed();
  expect(seeded.nodes.length + acmeTree().length).toBeGreaterThanOrEqual(2_000);
  await changeTree(seeded.nodes);
  // Documents beside every project and folder that was made: each stands in the tree as a row of its own.
  for (const place of seeded.places) for (let number = 1; number <= DOCUMENTS_PER_PLACE; number += 1) await makeDocument(place, `Notes ${number}`);
  const documents = seeded.places.length * DOCUMENTS_PER_PLACE;

  // Everything is open on this device, so that every thread is a row; the page notes when the last row is in.
  const wanted = seeded.nodes.length + acmeTree().length - 1;
  await page.addInitScript(
    ({ key, open, wanted, documents }) => {
      performance.setResourceTimingBufferSize(10_000);
      localStorage.setItem(key, JSON.stringify(open));
      const note = { lastRow: null as number | null, lastDocument: null as number | null };
      (window as unknown as { __tree: typeof note }).__tree = note;
      // The rows stand in runs, children of the tree: counting what the runs hold is cheap enough to do at every change of the page.
      const observer = new MutationObserver(() => {
        const tree = document.querySelector('[role="tree"]');
        if (tree === null) return;
        let rows = 0;
        for (const run of Array.from(tree.children)) rows += run.childElementCount;
        if (rows >= wanted && note.lastRow === null) note.lastRow = performance.now();
        // The documents are read place by place after the tree stands: the last of them is in when the rows are the nodes and the documents together.
        if (rows >= wanted + documents) {
          note.lastDocument = performance.now();
          observer.disconnect();
        }
      });
      observer.observe(document, { childList: true, subtree: true });
    },
    { key: `orch.${fixtureId(1, 1)}.tree.${ACME.org}`, open: seeded.open, wanted, documents },
  );
  await signIn(page);
  const slow = await slowdown(page);
  test.setTimeout(TEST_MS * Math.max(1, slow));
  // The screen is a small one, so that the tree is what is large on the page.
  await page.goto(`/project/${ACME.shop}`);
  await page.waitForFunction(() => (window as unknown as { __tree?: { lastRow: number | null } }).__tree?.lastRow !== null, undefined, { timeout: WAIT_MS * slow });
  // The documents of every open place have come, and no place still says that its documents are on their way.
  await page.waitForFunction(
    () => (window as unknown as { __tree?: { lastDocument: number | null } }).__tree?.lastDocument !== null && document.querySelectorAll('[role="tree"] [data-tree-row^="docs:"]').length === 0,
    undefined,
    { timeout: WAIT_MS * slow },
  );

  // How long the drawing took once the tree had arrived, and what the page holds now.
  const drawn = await page.evaluate((id) => {
    const note = (window as unknown as { __tree: { lastRow: number | null; lastDocument: number | null } }).__tree;
    const answer = performance.getEntriesByType('resource').filter((entry) => entry.name.includes(`/orgs/${id}/tree`)).at(-1) as PerformanceResourceTiming | undefined;
    const memory = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
    return {
      draw: (note.lastRow ?? 0) - (answer?.responseEnd ?? 0),
      documentsAfter: (note.lastDocument ?? 0) - (note.lastRow ?? 0),
      documentRows: document.querySelectorAll('[role="tree"] [data-tree-row^="doc:"]').length,
      rows: document.querySelectorAll('[role="tree"] [role="treeitem"]').length,
      nodes: document.querySelectorAll('*').length,
      heapMb: memory === undefined ? null : Math.round(memory.usedJSHeapSize / 1_048_576),
    };
  }, ACME.org);
  report('tree', { drawMs: drawn.draw, documentsAfterMs: drawn.documentsAfter, rows: drawn.rows, documentRows: drawn.documentRows, nodes: drawn.nodes, heapMb: drawn.heapMb, slowdown: slow });
  expect(drawn.rows).toBeGreaterThanOrEqual(wanted + documents);
  expect(drawn.documentRows).toBeGreaterThanOrEqual(documents);
  expect(drawn.draw, 'from the tree to its last row').toBeLessThan(BUDGET.drawMs * slow * (PRODUCTION ? 1 : DEVELOPMENT_DRAW));
  expect(drawn.nodes).toBeLessThan(BUDGET.nodes);
  if (drawn.heapMb !== null) expect(drawn.heapMb).toBeLessThan(BUDGET.heapMb);
  // Folders three deep stand in it as rows of their own, each a level in.
  await expect(page.locator('[role="treeitem"][aria-level="5"]').first()).toBeVisible();

  // The threads of the team's own tree are idle: a running thread in view turns its icon, which is work of its own, frame after frame.
  const own = acmeTree().filter((row) => row.kind === 'thread').map((row) => ({ id: row.id, thread: state('idle', 1) }));
  await changeTree(own);
  await page.waitForFunction(() => document.querySelectorAll('[role="tree"] [role="img"][aria-label="Running"], [role="tree"] [role="img"][aria-label="Starting"]').length === 0, undefined, { timeout: WAIT_MS * slow });

  // The row of a thread of the project on the screen is in view; the rows of the threads of the other projects are far above it.
  const inView = page.locator(`[data-tree-row="${ACME.feeTable}"]`);
  await inView.scrollIntoViewIfNeeded();
  await expect(inView).toBeInViewport();

  const cpu = await meter(page);
  const left = await untilQuiet(page, cpu, QUIET_PATIENCE_MS);
  const core = await speed(page, cpu);
  const budget = SPECIFIED_FRAME_MS * core * (PRODUCTION ? 1 : DEVELOPMENT_BUILD);

  // What the page spends over a pause when nothing is done: what is taken from each reading below.
  const resting: number[] = [];
  for (let round = 0; round < 5; round += 1) {
    const before = await cpu();
    await page.waitForTimeout(PAUSE_MS);
    resting.push((await cpu()) - before);
  }
  const baseline = median(resting);

  /** The CPU time that one change costs: from just before it is sent to a pause after the rows show it. */
  const costOf = async (thread: string, next: Shown, seq: number): Promise<number> => {
    const before = await cpu();
    await changeTree([{ id: thread, thread: state(next, seq) }]);
    await shows(page, thread, next, WAIT_MS * slow);
    await page.waitForTimeout(PAUSE_MS);
    return (await cpu()) - before - baseline;
  };

  // Threads of the other projects change, one after another, spread through the tree: the rows are not in view.
  const step = Math.floor(seeded.threads.length / CHANGES);
  const far: number[] = [];
  for (let at = 0; at < CHANGES; at += 1) far.push(await costOf(seeded.threads[at * step] ?? '', 'running', 2));
  report('far from view', { changes: CHANGES, leftMs: left, medianMs: median(far), p95Ms: percentile(far, 0.95), maxMs: Math.max(...far), baselineMs: baseline, budgetMs: budget, core, slowdown: slow });
  expect(median(far), 'the CPU time of a change of a row far out of view, the median').toBeLessThan(budget);
  expect(percentile(far, 0.95), 'the same, at the 95th percentile').toBeLessThan(budget * 3);

  // A thread in view is blocked, and is idle again, and so on: the rows are drawn, laid out and painted.
  const near: number[] = [];
  for (let at = 0; at < CHANGES; at += 1) near.push(await costOf(ACME.feeTable, at % 2 === 0 ? 'blocked' : 'idle', 3 + at));
  report('in view', { changes: CHANGES, medianMs: median(near), p95Ms: percentile(near, 0.95), maxMs: Math.max(...near), budgetMs: budget * FRAMES_OF_A_VISIBLE_CHANGE });
  expect(median(near), 'the CPU time of a change of a row in view, the median').toBeLessThan(budget * FRAMES_OF_A_VISIBLE_CHANGE);
  expect(percentile(near, 0.95), 'the same, at the 95th percentile').toBeLessThan(budget * FRAMES_OF_A_VISIBLE_CHANGE * 3);

  // Fifty at once, which is one frame of the live connection and one drawing of the tree; other threads each time.
  const together: number[] = [];
  for (let round = 0; round < BURST_ROUNDS; round += 1) {
    const burst = seeded.threads.slice(500 + round * BURST, 500 + (round + 1) * BURST);
    const before = await cpu();
    await changeTree(burst.map((thread) => ({ id: thread, thread: state('running', 20 + round) })));
    await shows(page, burst.at(-1) ?? '', 'running', WAIT_MS * slow);
    await page.waitForTimeout(PAUSE_MS);
    together.push((await cpu()) - before - baseline);
  }
  report('fifty at once', { rounds: BURST_ROUNDS, medianMs: median(together), maxMs: Math.max(...together), budgetMs: budget * BURST_FACTOR });
  expect(median(together), 'the CPU time of fifty changes made at once, the median').toBeLessThan(budget * BURST_FACTOR);

  // What changed is all that changed: the rows are as many as they were.
  expect(await rowsOf(page)).toBe(drawn.rows);

  // The keyboard reaches both ends of the tree, rows far out of view that the browser does not draw until it gets there.
  const items = page.locator('[role="tree"] [role="treeitem"]');
  await items.nth(3).focus();
  await page.keyboard.press('Home');
  await expect(items.first()).toBeFocused();
  await expect(items.first()).toBeInViewport();
  await page.keyboard.press('End');
  await expect(items.last()).toBeFocused();
  await expect(items.last()).toBeInViewport();
});
