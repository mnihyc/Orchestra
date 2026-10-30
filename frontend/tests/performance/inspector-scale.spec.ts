/** A directory of twenty thousand entries and a file of twenty thousand lines: how many rows the tree draws however long it is, how smoothly it scrolls, how soon a file is ready after its bytes arrive and how soon a key shows in it.
 *
 * What is measured, and how (a real browser, the development server, the fake API; the numbers are written into the report of each test):
 * - The tree: the rows drawn and all elements of the page with twenty thousand entries in one directory, the time from the click that
 *   opens the directory to its first rows, and the frames of a scroll of 150 frames at 200 pixels a frame taken from `requestAnimationFrame`.
 * - The file: from the moment the last byte of its content arrived (the end of the request, from the resource timings) to the first line drawn
 *   and the editor ready, and the time from a key to the character on screen (the key's own time stamp to the next mutation of the text).
 *
 * The budgets are what a reader would not notice on a machine busy with other work, with React's development build: set to fail for a page that has
 * become slow and not for a machine that has. The figures aimed at on a quiet machine are 500 ms for a file of two megabytes to be ready to scroll and
 * 16 ms for a key; the numbers here are far above them because the development build and a load of twenty on ten cores cost several times what a
 * production build on a quiet machine does, and they are held to what can be measured here: that the work does not grow with the size.
 */
import { expect, type Page, test } from '@playwright/test';

import { begin, editor, fileTree, MAIN, openThread, row, showView, workspace } from '../e2e/inspector-support.ts';

test.use({ viewport: { width: 1600, height: 1000 } });

const ENTRIES = 20_000;
const LINES = 20_000;

const BUDGET = {
  /** From the click on the directory to its first rows. */
  openTreeMs: 8_000,
  /** Rows of the tree drawn at any moment, and all elements of the page. */
  treeRows: 120,
  nodes: 9_000,
  /** A frame of the scroll: what stalls is a frame of seconds, which is what a row drawn again for every row of twenty thousand would cost. */
  frameP95Ms: 1_500,
  frameMaxMs: 4_000,
  /** From the last byte of a file's content to its first line and an editor that is ready. */
  fileReadyMs: 4_000,
  /** From a key to its character on screen. */
  keyMs: 1_000,
} as const;

test.beforeEach(async () => {
  await begin();
});

async function frames(page: Page, selector: string, count: number): Promise<{ p95: number; max: number }> {
  return page.evaluate(
    ([query, total]) =>
      new Promise<{ p95: number; max: number }>((resolve) => {
        const element = document.querySelector<HTMLElement>(query);
        const times: number[] = [];
        let last = performance.now();
        let frame = 0;
        const step = () => {
          const now = performance.now();
          times.push(now - last);
          last = now;
          if (element !== null) element.scrollTop += 200;
          frame += 1;
          if (frame < total) requestAnimationFrame(step);
          else {
            const sorted = [...times].sort((one, other) => one - other);
            resolve({ p95: sorted[Math.floor(sorted.length * 0.95)] ?? 0, max: sorted[sorted.length - 1] ?? 0 });
          }
        };
        requestAnimationFrame(step);
      }),
    [selector, count] as const,
  );
}

test('a directory of twenty thousand entries opens with a few rows drawn and scrolls without a frame that stalls', async ({ page }) => {
  await workspace('/fill', { directory: 'web/many', files: ENTRIES - 100, directories: 100 });
  await openThread(page);
  await showView(page, /^Files/);
  await expect(fileTree(page)).toBeVisible();
  const began = Date.now();
  await row(page, /^many/).click();
  // Directories come first, as the API lists them: the first rows are the folders.
  await expect(fileTree(page).getByRole('treeitem', { name: /^folder-0000/ })).toBeVisible({ timeout: 30_000 });
  const openMs = Date.now() - began;
  const held = await page.evaluate(() => ({ rows: document.querySelectorAll('[data-row-id]').length, nodes: document.querySelectorAll('*').length }));
  const scroll = await frames(page, '[data-testid="file-tree"]', 150);
  console.warn(`tree of ${ENTRIES} entries: open ${openMs} ms, ${held.rows} rows and ${held.nodes} elements, scroll p95 ${scroll.p95.toFixed(0)} ms, longest frame ${scroll.max.toFixed(0)} ms`);
  test.info().annotations.push({ type: 'measured', description: `open ${openMs} ms; ${held.rows} rows; ${held.nodes} elements; p95 ${scroll.p95.toFixed(0)} ms; max ${scroll.max.toFixed(0)} ms` });
  expect(openMs).toBeLessThan(BUDGET.openTreeMs);
  expect(held.rows).toBeLessThan(BUDGET.treeRows);
  expect(held.nodes).toBeLessThan(BUDGET.nodes);
  expect(scroll.p95).toBeLessThan(BUDGET.frameP95Ms);
  expect(scroll.max).toBeLessThan(BUDGET.frameMaxMs);
});

test('a file of twenty thousand lines is ready soon after its bytes arrive, and a key shows at once', async ({ page }) => {
  // The moments are taken in the page: when the content of the file finished arriving, and when its first line was drawn.
  await page.addInitScript(() => {
    const marks = { arrived: 0, first: 0 };
    (window as unknown as { __marks: typeof marks }).__marks = marks;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) if (entry.name.includes('/files/content')) marks.arrived = (entry as PerformanceResourceTiming).responseEnd;
    }).observe({ type: 'resource', buffered: true });
    new MutationObserver(() => {
      if (marks.first === 0 && document.querySelector('.cm-line') !== null) marks.first = performance.now();
    }).observe(document, { childList: true, subtree: true });
  });
  await workspace('/lines', { path: 'web/long.ts', lines: LINES });
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^long\.ts/).click();
  await expect(page.locator('.cm-line').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId('code-status')).toContainText('Line 1, column 1');
  const marks = await page.evaluate(() => (window as unknown as { __marks: { arrived: number; first: number } }).__marks);
  const readyMs = Math.max(0, marks.first - marks.arrived);

  // A key on screen: from the key's own time stamp to the mutation of the text that shows it.
  await editor(page).click();
  await page.keyboard.press('Control+Home');
  const pending = page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const content = document.querySelector('.cm-content');
        let stamp = 0;
        document.addEventListener('keydown', (event) => (stamp = event.timeStamp), { once: true, capture: true });
        new MutationObserver((_records, watch) => {
          watch.disconnect();
          resolve(performance.now() - stamp);
        }).observe(content ?? document, { childList: true, subtree: true, characterData: true });
      }),
  );
  await page.waitForTimeout(100);
  await page.keyboard.press('a');
  const keyMs = await pending;
  console.warn(`file of ${LINES} lines: first line ${readyMs.toFixed(0)} ms after its content arrived, a key on screen after ${keyMs.toFixed(0)} ms`);
  test.info().annotations.push({ type: 'measured', description: `ready ${readyMs.toFixed(0)} ms after the bytes; key ${keyMs.toFixed(0)} ms` });
  expect(readyMs).toBeLessThan(BUDGET.fileReadyMs);
  expect(keyMs).toBeLessThan(BUDGET.keyMs);
  expect(MAIN).toBeDefined();
});
