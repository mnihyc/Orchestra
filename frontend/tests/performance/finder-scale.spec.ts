/** The finder over ten thousand names: how soon what the page holds is listed after a key, and how soon the names the server finds are.
 *
 * What is measured, and how (a real browser, the development server, the fake API holding ten thousand names besides the team's own):
 * - Local groups (commands, settings, help): in the page, from the `input` event of the field to the moment a row that
 *   answers is in the list, taken by a MutationObserver on the list. The budget of the product is 50 ms for it, with the
 *   production build; these tests run React's development build, which draws a row several times slower, and
 *   the budget here is for that.
 * - Named things: the same, for a name that only one of the ten thousand has. It includes the pause of 150 ms the
 *   finder waits after the last key, the request, and the drawing. The budget of the product is 300 ms on a LAN.
 * The budgets in milliseconds are for a quiet machine and are multiplied by how much slower than a quiet one this machine is
 * just now (a fixed piece of work is timed in the page), so that a busy machine is not taken for a slow finder. The figures
 * measured are written into the report of the run.
 */
import { expect, type Page, test } from '@playwright/test';

import { fillNames, scenario, signIn } from '../e2e/support.ts';

/** What a fixed piece of work (twenty million square roots) takes on a core with nothing else to do. */
const QUIET_WORK_MS = 40;
const SLOWDOWN_AT_MOST = 12;
/** What the finder may take on a quiet machine with React's development build: the product's 50 ms and 300 ms, with room for the build. */
const LOCAL_BUDGET_MS = 250;
const NAMED_BUDGET_MS = 600;
const SAMPLES = 5;

async function slowdown(page: Page): Promise<number> {
  const took = await page.evaluate(() => {
    const start = performance.now();
    let sum = 0;
    for (let index = 0; index < 20_000_000; index += 1) sum += Math.sqrt(index);
    return performance.now() - start + (sum < 0 ? 1 : 0);
  });
  return Math.min(SLOWDOWN_AT_MOST, Math.max(1, took / QUIET_WORK_MS));
}

/** Types into the field and times, in the page, how long it takes for a row that matches `expected` to stand in the list. */
async function timeTo(page: Page, text: string, expected: string): Promise<number> {
  const field = page.getByRole('dialog', { name: 'Find' }).getByRole('combobox', { name: 'Search terms', exact: true });
  await field.fill('');
  await page.evaluate(
    (pattern) => {
      (window as unknown as { measured: Promise<number> }).measured = new Promise<number>((resolve) => {
        const input = document.querySelector<HTMLInputElement>('[cmdk-input]');
        const list = document.querySelector<HTMLElement>('[cmdk-list]');
        if (input === null || list === null) {
          resolve(-1);
          return;
        }
        const wanted = new RegExp(pattern, 'i');
        let began: number | null = null;
        input.addEventListener('input', () => (began = performance.now()), { once: true });
        const observer = new MutationObserver(() => {
          if (began !== null && wanted.test(list.innerText)) {
            observer.disconnect();
            resolve(performance.now() - began);
          }
        });
        observer.observe(list, { childList: true, subtree: true, characterData: true });
        setTimeout(() => {
          observer.disconnect();
          resolve(-1);
        }, 20_000);
      });
    },
    expected,
  );
  await field.fill(text);
  return page.evaluate(() => (window as unknown as { measured: Promise<number> }).measured);
}

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? 0;
const slowest = (values: number[]) => Math.max(...values);

test('the finder answers within its budget with ten thousand names in the fake API', async ({ page }, info) => {
  test.setTimeout(240_000);
  await scenario();
  await fillNames(10_000);
  await signIn(page);
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestIdleCallback(() => resolve(), { timeout: 2_000 }))));
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: 'Find' })).toBeVisible();
  // The registry and the guide are read once, at the first letter: they are part of how soon the first answer comes, not of the next ones.
  await page.getByRole('dialog', { name: 'Find' }).getByRole('combobox', { name: 'Search terms', exact: true }).fill('idle');
  await expect(page.getByRole('option', { name: /Idle stop/ })).toBeVisible();

  const factor = await slowdown(page);

  const local: number[] = [];
  for (const [text, expected] of [['>sign', 'Sign out'], ['>theme', 'Theme: Dark'], [':sandbox', 'Sandbox'], ['>new org', 'New org'], [':idle stop', 'Idle stop']] as const) {
    local.push(await timeTo(page, text, expected));
  }
  const named: number[] = [];
  for (const number of ['00042', '07777', '09999', '01234', '05000']) {
    // One name of ten thousand has this number. The row that offers to search for the words typed holds the number too, and is
    // there at once: what is waited for is the name itself ("Quarterly report 00042"), which only the server's answer holds.
    named.push(await timeTo(page, `report ${number}`, `(Quarterly|Nightly|Notes on) report ${number}`));
  }

  const report = `slowdown ${factor.toFixed(1)}x; local groups: median ${median(local).toFixed(0)} ms, slowest ${slowest(local).toFixed(0)} ms (budget ${(LOCAL_BUDGET_MS * factor).toFixed(0)} ms); named things: median ${median(named).toFixed(0)} ms, slowest ${slowest(named).toFixed(0)} ms (budget ${(NAMED_BUDGET_MS * factor).toFixed(0)} ms)`;
  console.warn(report);
  info.annotations.push({ type: 'figures', description: report });
  expect(SAMPLES).toBe(local.length);
  for (const sample of local) expect(sample, report).toBeGreaterThan(0);
  for (const sample of named) expect(sample, report).toBeGreaterThan(0);
  expect(slowest(local), report).toBeLessThan(LOCAL_BUDGET_MS * factor);
  expect(slowest(named), report).toBeLessThan(NAMED_BUDGET_MS * factor);
});
