// Thirty separate real browser processes against the candidate's own frontend. No route or socket mocking.
import { createRequire } from 'node:module';
import { readFile, writeFile, access } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(resolve(here, '../../../frontend/package.json'));
const { chromium } = require('@playwright/test');
const [worldPath, out, budgetPath] = process.argv.slice(2);
if (!worldPath || !out || !budgetPath) throw new Error('world, output directory and budget file are required');
const world = JSON.parse(await readFile(worldPath, 'utf8'));
const budgets = JSON.parse(await readFile(budgetPath, 'utf8'));
const instances = [];
const results = [];
const broken = [];
const waitFile = async (name) => {
  const end = performance.now() + 300_000; // Hang guard; never a performance assertion.
  while (true) {
    try { await access(resolve(out, name)); return; } catch { /* The producing process has not published it. */ }
    if (performance.now() >= end) throw new Error(`missing ${name}`);
    await new Promise((done) => setTimeout(done, 100));
  }
};

function observe() {
  window.proof = { deltas: [], keys: [] };
  const Original = window.WebSocket;
  window.WebSocket = class extends Original {
    constructor(...args) {
      super(...args);
      this.addEventListener('message', (message) => {
        let frame;
        try { frame = JSON.parse(message.data); } catch { return; }
        if (frame.op !== 'delta' || frame.field !== 'text' || !frame.data) return;
        const start = performance.now();
        const check = () => {
          const row = document.querySelector(`[data-item="${frame.item_id}"]`);
          if (row?.textContent?.includes(frame.data) && row.getBoundingClientRect().height > 0) {
            requestAnimationFrame(() => window.proof.deltas.push(performance.now() - start));
            return;
          }
          if (performance.now() - start < 5000) requestAnimationFrame(check);
          else window.proof.deltas.push(5000);
        };
        requestAnimationFrame(check);
      });
    }
  };
  document.addEventListener('input', (event) => {
    if (!(event.target instanceof HTMLTextAreaElement) || !event.target.closest('[data-composer]')) return;
    const start = performance.now();
    requestAnimationFrame(() => window.proof.keys.push(performance.now() - start));
  }, true);
}

async function ready(page) {
  await page.waitForFunction(() => {
    const box = document.querySelector('[data-composer] textarea');
    return box !== null && !box.disabled && document.querySelector('[role="log"]') !== null;
  }, undefined, { timeout: 60_000 });
}

try {
  await waitFile('ready');
  const threads = JSON.parse(await readFile(resolve(out, 'threads.json'), 'utf8'));
  // Warm and authenticate in small batches; the measured navigation below is simultaneous.
  for (let start = 0; start < 30; start += 5) {
    await Promise.all(Array.from({ length: 5 }, async (_, offset) => {
      const index = start + offset;
      const browser = await chromium.launch({ headless: true,
        args: ['--host-resolver-rules=MAP proof.orch.test 127.0.0.1', '--disable-dev-shm-usage'] });
      instances.push(browser);
      const context = await browser.newContext({ ignoreHTTPSErrors: true, locale: 'en-US' });
      await context.addInitScript(observe);
      const page = await context.newPage();
      page.on('pageerror', (error) => broken.push({ browser: index, message: error.message }));
      await page.goto(`${world.system}/login`);
      await page.locator('input[autocomplete="username"]').fill('ada');
      await page.locator('input[autocomplete="current-password"]').fill('correct horse battery');
      await page.getByRole('button', { name: 'Sign in', exact: true }).click();
      await page.waitForURL((url) => url.pathname !== '/login');
      await page.goto(`${world.system}/thread/${threads[index]}`);
      await ready(page);
      results[index] = { index, browser, page };
    }));
  }
  await writeFile(resolve(out, 'browsers-ready'), '30 independent Chromium processes ready');
  await waitFile('streaming');
  const measured = await Promise.all(results.map(async ({ index, browser, page }) => {
    const started = performance.now();
    await page.reload({ waitUntil: 'domcontentloaded' });
    await ready(page);
    const warm = performance.now() - started;
    await page.locator('[data-composer] textarea').pressSequentially('a local draft', { delay: 20 });
    await page.waitForFunction(() => window.proof.deltas.length > 0, undefined, { timeout: 300_000 });
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Performance.enable');
    const metrics = await cdp.send('Performance.getMetrics');
    const heap = metrics.metrics.find((value) => value.name === 'JSHeapUsedSize')?.value;
    if (heap === undefined) throw new Error('Chromium did not supply renderer memory');
    const observed = await page.evaluate(() => window.proof);
    const delta = Math.max(...observed.deltas);
    const key = Math.max(...observed.keys);
    if (observed.keys.length === 0) throw new Error('no real composer input was observed');
    return { browser: index, warm_thread_ms: warm, renderer_heap_bytes: heap,
      delta_to_next_paint_ms: delta, typing_to_next_paint_ms: key,
      delta_samples: observed.deltas.length, typing_samples: observed.keys.length,
      passed: warm <= budgets.warm_thread_ms && heap <= budgets.renderer_heap_bytes &&
        delta <= budgets.delta_paint_ms && key <= budgets.typing_paint_ms };
  }));
  const result = { browser_count: 30, process_isolation: 'one Chromium launch per browser',
    real_frontend: true, mocked_requests: 0, rows: measured, page_errors: broken,
    passed: measured.length === 30 && measured.every((row) => row.passed) && broken.length === 0 };
  await writeFile(resolve(out, 'browsers-result.json'), JSON.stringify(result, null, 2));
  if (!result.passed) throw new Error('the measured browsers exceeded a budget or had a page error');
} catch (error) {
  await writeFile(resolve(out, 'browsers-failure.json'), JSON.stringify({ passed: false, message: String(error) }));
  throw error;
} finally {
  await Promise.all(instances.map((browser) => browser.close()));
}
