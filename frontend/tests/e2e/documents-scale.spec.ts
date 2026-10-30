/** A page of two thousand blocks: opened, typed in, and changed by another person, within budgets that can be measured here.
 *
 * The budgets and how each is measured (all in the page, with the clock of its own performance timeline,
 * so that the test's own round trips are not in them):
 *
 * - Opened: from the moment the page asks for the document to the moment its first block is drawn.
 * - Typing: for each key, from the `keydown` to the paint that follows the change it made to the page
 *   (a mutation of the page, then two animation frames). The median and the slowest of thirty keys.
 * - Another person's change: from the request that makes it to the moment its words are in the page, and
 *   the longest task of the page's main thread while it arrives and the person types.
 *
 * This machine is shared and often busy: the numbers are about three times what a quiet one gives, and the
 * budgets leave that room. They are the line a regression would cross; the numbers seen are in the test's
 * annotations and in the notes.
 */
import { expect, test } from '@playwright/test';

import { scenario, signIn } from './support.ts';
import { actAs, editorOf, held, makeLong, waitUntilSaved } from './documents-support.ts';

/** How long from asking for a page of two thousand blocks to its first block, at most. */
const OPEN_BUDGET_MS = 6_000;
/** A key's median and slowest time from the press to the paint, at most. */
const KEY_MEDIAN_BUDGET_MS = 120;
const KEY_SLOWEST_BUDGET_MS = 500;
/** How long another person's change takes to be in the page, at most. */
const REMOTE_BUDGET_MS = 3_000;
/** The longest stretch the page's main thread may be held while others write and the person types, at most. */
const LONGEST_TASK_BUDGET_MS = 600;

const BLOCKS = 2_000;

test.beforeEach(async () => {
  await scenario();
});

test('a page of two thousand blocks is opened, typed in and changed by another person within the budgets', async ({ page }) => {
  test.setTimeout(240_000);
  await signIn(page);
  const doc = await makeLong(BLOCKS, 'Two thousand blocks');

  // What is measured in the page is set up before the page's own scripts run.
  await page.addInitScript(() => {
    // The page loads some hundreds of files (a development server hands over modules, a build its chunks) before it asks for the
    // document: the default room for entries, 250, would be full, and the request for the document would not be among them.
    performance.setResourceTimingBufferSize(10_000);
    const w = window as unknown as { __first: number | null; __long: number[]; __keys: number[] };
    w.__first = null;
    w.__long = [];
    w.__keys = [];
    new MutationObserver((_records, observer) => {
      if (document.querySelector('.doc-editor [data-ot]') !== null) {
        w.__first = performance.now();
        observer.disconnect();
      }
    }).observe(document, { childList: true, subtree: true });
    try {
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) w.__long.push(entry.duration);
      }).observe({ type: 'longtask', buffered: false });
    } catch {
      // A browser without long-task timing leaves the list empty; the test then has nothing to compare.
    }
  });

  await page.goto(`/doc/${doc}`);
  await expect(editorOf(page)).toBeVisible();
  await expect(page.locator('.doc-editor [data-ot]').first()).toBeVisible();
  const opened = await page.evaluate(() => {
    const w = window as unknown as { __first: number | null };
    const asked = performance.getEntriesByType('resource').find((entry) => entry.name.includes('/sync'));
    return w.__first === null || asked === undefined ? Number.NaN : w.__first - asked.startTime;
  });
  expect(opened, 'the time to the first block, in ms').toBeLessThan(OPEN_BUDGET_MS);

  // The whole page is there: the first block and the last.
  await expect(editorOf(page).locator('[data-ot="paragraph"]')).toHaveCount(BLOCKS);

  // Typing in the middle of the page: put the caret in a block that is on screen after scrolling there.
  await page.evaluate(() => {
    const scroller = document.querySelector('[data-document-scroll]');
    if (scroller !== null) scroller.scrollTop = scroller.scrollHeight / 2;
  });
  await page.waitForTimeout(500);
  const middle = page.locator('.doc-editor [data-ot="paragraph"]').filter({ hasText: /Paragraph 1[0-9]{3}:/ }).first();
  await middle.scrollIntoViewIfNeeded();
  await middle.click();
  await page.keyboard.press('End');
  await page.evaluate(() => {
    const w = window as unknown as { __long: number[]; __keys: number[] };
    w.__long = [];
    w.__keys = [];
    const editor = document.querySelector('.doc-editor');
    document.addEventListener(
      'keydown',
      () => {
        const pressed = performance.now();
        if (editor === null) return;
        const watch = new MutationObserver(() => {
          watch.disconnect();
          requestAnimationFrame(() => requestAnimationFrame(() => w.__keys.push(performance.now() - pressed)));
        });
        watch.observe(editor, { childList: true, subtree: true, characterData: true });
      },
      true,
    );
  });
  await page.keyboard.type('the quick brown fox jumps over', { delay: 40 });
  await waitUntilSaved(page);
  const keys = await page.evaluate(() => (window as unknown as { __keys: number[] }).__keys);
  expect(keys.length, 'keys timed').toBeGreaterThanOrEqual(25);
  const sorted = [...keys].sort((one, other) => one - other);
  const median = sorted[Math.floor(sorted.length / 2)] ?? Number.NaN;
  const slowest = sorted[sorted.length - 1] ?? Number.NaN;
  expect(median, 'median key to paint, in ms').toBeLessThan(KEY_MEDIAN_BUDGET_MS);
  expect(slowest, 'slowest key to paint, in ms').toBeLessThan(KEY_SLOWEST_BUDGET_MS);

  // What was typed was sent as one block's change, not as a page.
  const now = await held(doc);
  expect(now.blocks.filter((one) => one.seq_updated > 1).length, 'blocks written for the typing').toBeLessThanOrEqual(2);

  // Another person writes in a block far from the caret while the person goes on typing: it arrives, and nothing typed is lost.
  const far = now.blocks[BLOCKS - 5]!;
  const started = Date.now();
  const typing = page.keyboard.type(' and then keeps on', { delay: 60 });
  await actAs('dora', doc, [{ op: 'update', id: far.id, base_seq: far.seq_updated, content: [{ type: 'text', text: 'Written by Dora, far away.' }] }]);
  await expect(page.locator(`[data-id="${far.id}"]`)).toContainText('Written by Dora, far away.');
  const remote = Date.now() - started;
  await typing;
  await waitUntilSaved(page);
  expect(remote, 'another person\'s change to be in the page, in ms').toBeLessThan(REMOTE_BUDGET_MS);
  const longest = await page.evaluate(() => Math.max(0, ...(window as unknown as { __long: number[] }).__long));
  expect(longest, 'longest task of the main thread, in ms').toBeLessThan(LONGEST_TASK_BUDGET_MS);
  await expect(middle).toContainText('the quick brown fox jumps over and then keeps on');

  test.info().annotations.push({
    type: 'budgets',
    description: `${BLOCKS} blocks: first block ${opened.toFixed(0)} ms after the request (budget ${OPEN_BUDGET_MS}); key to paint median ${median.toFixed(0)} ms, slowest ${slowest.toFixed(0)} ms (budgets ${KEY_MEDIAN_BUDGET_MS}, ${KEY_SLOWEST_BUDGET_MS}); another person's change in the page after ${remote} ms (budget ${REMOTE_BUDGET_MS}); longest main-thread task ${longest.toFixed(0)} ms (budget ${LONGEST_TASK_BUDGET_MS}).`,
  });
  // The numbers are for the notes: they are printed too.
  process.stdout.write(`${test.info().annotations.at(-1)?.description ?? ''}\n`);
});
