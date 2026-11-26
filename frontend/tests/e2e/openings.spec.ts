/** The first page a person opens shows its content within a second at a 200 ms round trip, from one request that restores
 * the session: Home, the resource lists and Settings like a thread. Timed in the page itself against its own content mark,
 * over the transport the shipped edge gives (`h2-origin.ts`). The sign-in page's own figure is the shipped edge's case in
 * `first-wave-edge.spec.ts`: its words and its meta come with the document there, which no other server here gives. */
import { resolve } from 'node:path';
import { chromium, expect, type Browser, type CDPSession, type Page, test } from '@playwright/test';

import { h2Origin } from './h2-origin';
import { ACME, DOUBLE, PEOPLE, scenario, signIn } from './support';

const ROUND_TRIP_MS = 200;
const BAR_MS = 1_000;

interface Sent { method: string; path: string; query: string; wall: number; failed?: string }
interface Fetched { path: string; wall: number }

/** Every request of the page from now on, by the browser's own clock, with what ended it early. */
async function watch(page: Page): Promise<{ cdp: CDPSession; sent: Sent[]; files: Fetched[] }> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  const sent: Sent[] = [], files: Fetched[] = [];
  const byId = new Map<string, Sent>();
  cdp.on('Network.requestWillBeSent', ({ requestId, request, wallTime }) => {
    const url = new URL(request.url);
    if (!url.pathname.startsWith('/api/')) { files.push({ path: url.pathname, wall: wallTime * 1000 }); return; }
    if (!url.pathname.startsWith('/api/v1/') || url.pathname === '/api/v1/ws') return;
    const one = { method: request.method, path: url.pathname, query: url.search, wall: wallTime * 1000 };
    byId.set(requestId, one); sent.push(one);
  });
  cdp.on('Network.loadingFailed', ({ requestId, errorText, canceled }) => {
    const one = byId.get(requestId);
    if (one !== undefined) one.failed = canceled === true ? 'canceled' : errorText;
  });
  return { cdp, sent, files };
}

/** The document and each file fetched before the content, by when it was asked after the document. */
function waves(files: readonly Fetched[], until: number): string[] {
  const start = files[0]?.wall ?? 0;
  return files.filter((file) => file.wall <= until).map((file) => `${Math.round(file.wall - start)} ${file.path.replace(/-[A-Za-z0-9_-]{8}(\.[a-z0-9]+)$/, '$1')}`);
}

async function longLine(cdp: CDPSession): Promise<void> {
  await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: ROUND_TRIP_MS, downloadThroughput: -1, uploadThroughput: -1 });
}

/** When the page's own content mark said its content is shown, on the wall clock, and where. */
async function contentShown(page: Page, address: RegExp | null): Promise<{ ms: number; wall: number; address: string }> {
  const handle = await page.waitForFunction((source) => {
    const pattern = source === null ? null : new RegExp(source);
    for (const entry of performance.getEntriesByName('orch/page/content', 'mark')) {
      const detail = (entry as PerformanceMark).detail as { address?: string; outcome?: string } | null;
      if (detail?.outcome === 'ready' && (pattern === null || pattern.test(detail.address ?? ''))) {
        return { ms: entry.startTime, wall: performance.timeOrigin + entry.startTime, address: detail.address ?? '' };
      }
    }
    return false;
  }, address === null ? null : address.source, { polling: 'raf', timeout: 60_000 });
  return await handle.jsonValue() as { ms: number; wall: number; address: string };
}

/**
 * The native server sends a root's settings only with an unscoped first page of its tree, and every read of the left bar is
 * scoped (its order and parent): the bar then reads the root's settings by themselves, as it does on an installation.
 */
async function nativeTreeReplies(page: Page): Promise<void> {
  await page.route(/\/api\/v1\/orgs\/[^/]+\/tree\?/, async (route) => {
    const url = new URL(route.request().url());
    const response = await route.fetch();
    if (!response.ok() || !['parent_id', 'path_to', 'sort', 'q', 'kind', 'below_id'].some((name) => url.searchParams.has(name))) {
      await route.fulfill({ response });
      return;
    }
    const body = await response.json() as Record<string, unknown>;
    delete body.settings;
    await route.fulfill({ response, json: body });
  });
}

interface TreeRead { query: string; outcome: string }

/**
 * The tree reads of the next document as its own fetch sees them: each read's query and what became of it, its answer's
 * status, `let go` once the page aborts the read's signal (at any point: every way the page lets a read go aborts it), or
 * `failed`. The browser's network record cannot say whether the page let a read go: under the emulated round trip it
 * reports reads canceled whose whole answer had arrived and been taken (every byte received, nothing of the page aborted,
 * the branch's own rows read on at once): 4-8 of an opening's 10 with the replies as the fake API sends them, 0-2 with
 * the native ones, none without the emulated round trip.
 */
async function pageTreeReads(page: Page): Promise<() => Promise<TreeRead[]>> {
  await page.addInitScript(() => {
    const reads: { query: string; outcome: string }[] = [];
    Object.defineProperty(window, '__orchTreeReads', { value: reads });
    const fetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (!/^\/api\/v1\/orgs\/[^/]+\/tree$/.test(url.pathname)) return fetch(input, init);
      const read = { query: url.search, outcome: 'asked' };
      reads.push(read);
      (init?.signal ?? (input instanceof Request ? input.signal : null))?.addEventListener('abort', () => { read.outcome = 'let go'; }, { once: true });
      const answer = fetch(input, init);
      void answer.then((response) => { if (read.outcome === 'asked') read.outcome = String(response.status); },
        () => { if (read.outcome === 'asked') read.outcome = 'failed'; });
      return answer;
    };
  });
  return async () => await page.evaluate(() => (window as Window & { __orchTreeReads?: TreeRead[] }).__orchTreeReads ?? []);
}

const shown = (requests: readonly Sent[]) => requests.map((one) => `${one.method} ${one.path}${one.query}${one.failed === undefined ? '' : ` (${one.failed})`}`);

const PAGES: { name: string; address: string; content: RegExp | null }[] = [
  { name: 'Home', address: '/', content: null },
  { name: 'Providers', address: '/resources/providers', content: /^\/resources\/providers$/ },
  { name: 'Machines', address: '/resources/machines', content: /^\/resources\/machines$/ },
  { name: 'Settings', address: '/settings', content: /^\/settings(?:\/|$)/ },
];

test.beforeEach(async () => { await scenario(); });

/**
 * A page of the built application served over h2, closed with the test. The browser is one of its own that accepts the
 * test's certificate everywhere, the application's worker included (a context's own setting covers pages, not a worker's
 * script), so that the page is the one a person gets from the edge.
 */
async function overH2(run: (page: Page) => Promise<void>): Promise<void> {
  const origin = await h2Origin(resolve(process.cwd(), 'dist'), new URL(DOUBLE), test.info().outputPath('h2-origin'));
  let browser: Browser | undefined;
  try {
    browser = await chromium.launch({ args: ['--disable-gpu', '--ignore-certificate-errors'] });
    const context = await browser.newContext({ baseURL: origin.origin, ignoreHTTPSErrors: true, locale: 'en-US', timezoneId: 'UTC',
      viewport: { width: 1440, height: 900 } });
    await run(await context.newPage());
  } finally { await browser?.close(); await origin.close(); }
}

for (const { name, address, content } of PAGES.filter((entry) => entry.name !== 'Settings')) {
  test(`${name} opens from one request after the previous tab was closed at its first heading`, async () => overH2(async (page) => {
    const login = await page.request.post('/api/v1/auth/login', { data: PEOPLE.ada });
    expect(login.status()).toBe(200);
    const context = page.context();
    await longLine((await watch(page)).cdp);
    await page.goto(address, { waitUntil: 'commit' });
    await page.locator('main h1, main h2, [data-main-area] h1').first().waitFor();
    await page.close();

    const returning = await context.newPage();
    const { cdp, sent } = await watch(returning);
    await longLine(cdp);
    await returning.goto(address, { waitUntil: 'commit' });
    const drawn = await contentShown(returning, content ?? /^\/org\/[^/]+\/?$/);
    const before = sent.filter((one) => one.wall <= drawn.wall);
    process.stdout.write(`${name} after closing at the heading, ${ROUND_TRIP_MS} ms RTT: content ${Math.round(drawn.ms)} ms; requests ${JSON.stringify(shown(before))}\n`);
    expect(shown(before)).toEqual([expect.stringMatching(/^POST \/api\/v1\/auth\/session\/open\?read=%2Fme%2Fbootstrap/)]);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  }));
}

for (const { name, address, content } of PAGES) {
  test(`${name} opens again within a second at 200 ms from one request that restores the session`, async () => overH2(async (page) => {
    await signIn(page);
    // The person was here before: the device has the page's code and knows what the page reads first.
    await page.goto(address);
    await contentShown(page, content);
    await page.waitForLoadState('networkidle');
    const { cdp, sent } = await watch(page);
    await longLine(cdp);
    // What the page before it still asked (a branch of the left bar after its content) is not this opening's.
    const opened = Date.now();
    await page.reload({ waitUntil: 'commit' });
    const drawn = await contentShown(page, content);
    const before = sent.filter((one) => one.wall >= opened && one.wall <= drawn.wall);
    process.stdout.write(`${name} warm at ${ROUND_TRIP_MS} ms: content ${Math.round(drawn.ms)} ms at ${drawn.address}; requests before it: ${JSON.stringify(shown(before))}\n`);
    expect(shown(before)).toEqual([expect.stringMatching(/^POST \/api\/v1\/auth\/session\/open\?read=%2Fme%2Fbootstrap/)]);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  }));

  test(`${name} opens within a second at 200 ms with nothing of its code in the browser's cache`, async () => overH2(async (page) => {
    await signIn(page);
    await page.goto(address);
    await contentShown(page, content);
    await page.waitForLoadState('networkidle');
    const { cdp, sent, files } = await watch(page);
    await cdp.send('Network.clearBrowserCache');
    await longLine(cdp);
    await page.addInitScript(() => {
      const kept: string[] = [];
      (window as Window & { __orchLongTasks?: string[] }).__orchLongTasks = kept;
      new PerformanceObserver((list) => { for (const entry of list.getEntries()) kept.push(`${Math.round(entry.startTime)}+${Math.round(entry.duration)}`); })
        .observe({ type: 'longtask', buffered: true });
    });
    const opened = Date.now();
    await page.goto(address, { waitUntil: 'commit' });
    const drawn = await contentShown(page, content);
    const before = sent.filter((one) => one.wall >= opened && one.wall <= drawn.wall);
    process.stdout.write(`${name} cold at ${ROUND_TRIP_MS} ms: content ${Math.round(drawn.ms)} ms at ${drawn.address}; requests before it: ${JSON.stringify(shown(before))}; files: ${JSON.stringify(waves(files, drawn.wall))}\n`);
    // The page's own milestones and its main thread's long work, to see what the drawing after the data spends.
    const phases = await page.evaluate(() => ({
      marks: performance.getEntriesByType('mark').filter((entry) => entry.name.startsWith('orch/')).map((entry) => `${Math.round(entry.startTime)} ${entry.name}`),
      tasks: ((window as Window & { __orchLongTasks?: string[] }).__orchLongTasks ?? []),
      // When each file of the first wave was asked for and when its last byte was in: what the drawing waited for.
      files: performance.getEntriesByType('resource').filter((entry) => /\/(?:assets|fonts)\//.test(entry.name))
        .map((entry) => entry as PerformanceResourceTiming)
        .map((entry) => `${Math.round(entry.startTime)}-${Math.round(entry.responseEnd)} ${Math.round(entry.encodedBodySize / 1024)}k ${new URL(entry.name).pathname}`) }));
    process.stdout.write(`${name} cold phases: ${JSON.stringify(phases)}\n`);
    expect(shown(before)).toEqual([expect.stringMatching(/^POST \/api\/v1\/auth\/session\/open\?read=%2Fme%2Fbootstrap/)]);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  }));
}

/**
 * A place page (Home lands on one) shows its content without waiting for the reads beside it: on an installation the
 * place's chat opening took 2.2-4.2 s and its binding options 1.8 s, so the case gives them that long here.
 */
for (const { name, address, content } of [
  { name: 'Home', address: '/', content: null },
  { name: 'A topic', address: `/topic/${ACME.fixLogin}`, content: /^\/topic\// },
]) {
  test(`${name} shows its content within a second at 200 ms while its chat and binding reads are still on their way`, async () => overH2(async (page) => {
    await signIn(page);
    await page.goto(address);
    await contentShown(page, content);
    await page.waitForLoadState('networkidle');
    await fetch(`${DOUBLE}/__double/read-delays`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ delays: { '/chat-opening': 2_500, '/binding/options': 1_800 } }) });
    const { cdp } = await watch(page);
    await longLine(cdp);
    await page.reload({ waitUntil: 'commit' });
    const drawn = await contentShown(page, content);
    process.stdout.write(`${name} beside slow reads at ${ROUND_TRIP_MS} ms: content ${Math.round(drawn.ms)} ms at ${drawn.address}\n`);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  }));
}

test('signing in shows the saved conversation within a second of the login answer at 200 ms', async () => overH2(async (page) => {
  await fetch(`${DOUBLE}/__double/first-steps/landing`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ place: { node_id: ACME.mainThread, route: `/thread/${ACME.mainThread}` } }) });
  const { cdp, sent, files } = await watch(page);
  await longLine(cdp);
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  const from = sent.length, fromFile = files.length;
  await page.getByRole('button', { name: 'Sign in' }).click();
  const landed = await page.waitForFunction(() => {
    const login = performance.getEntriesByType('resource').find((entry) => new URL(entry.name).pathname === '/api/v1/auth/login') as PerformanceResourceTiming | undefined;
    const header = document.querySelector('[data-thread-header]');
    const composer = document.querySelector<HTMLTextAreaElement>('[data-composer] textarea');
    if (login === undefined || !header?.getBoundingClientRect().height || !composer?.getBoundingClientRect().height || composer.disabled) return false;
    return { ms: performance.now() - login.responseEnd, wall: performance.timeOrigin + performance.now(), answered: performance.timeOrigin + login.responseEnd };
  }, undefined, { polling: 'raf', timeout: 60_000 });
  const drawn = await landed.jsonValue() as { ms: number; wall: number; answered: number };
  const after = sent.slice(from).filter((one) => one.wall <= drawn.wall);
  process.stdout.write(`Signed-in landing at ${ROUND_TRIP_MS} ms: ${Math.round(drawn.ms)} ms after the login answer; requests: ${JSON.stringify(shown(after))}; files after the answer: ${JSON.stringify(waves(files.slice(fromFile).filter((file) => file.wall >= drawn.answered - ROUND_TRIP_MS), drawn.wall))}\n`);
  expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
}));

test('a first opening reads each part of the navigation tree once and lets none of them go', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await nativeTreeReplies(page);
  await signIn(page);
  const { cdp, sent } = await watch(page);
  const treeReads = await pageTreeReads(page);
  await longLine(cdp);
  await page.goto('/resources/providers');
  await contentShown(page, /^\/resources\/providers$/);
  await page.waitForLoadState('networkidle');
  const reads = await treeReads();
  const reported = new Set(sent.filter((one) => /^\/api\/v1\/orgs\/[^/]+\/tree$/.test(one.path) && one.failed !== undefined).map((one) => one.query));
  process.stdout.write(`Tree reads of a first opening: ${JSON.stringify(reads.map((read) => `${read.query} ${read.outcome}${reported.has(read.query) ? ' (reported canceled by the browser)' : ''}`))}\n`);
  expect(reads.length).toBeGreaterThan(0);
  expect(reads.filter((read) => read.outcome !== '200')).toEqual([]);
  expect(new Set(reads.map((read) => read.query)).size).toBe(reads.length);
});

test('the first switch to a tab restored after a reload asks for no read the page already holds', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await nativeTreeReplies(page);
  await signIn(page);
  await page.goto(`/thread/${ACME.repro}`);
  await expect(page.locator('[data-screen="thread"] h1').filter({ hasText: 'Repro' }).first()).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-screen="thread"] h1').filter({ hasText: 'Main thread' }).first()).toBeVisible();
  await page.waitForLoadState('networkidle');
  const { cdp, sent } = await watch(page);
  await longLine(cdp);
  await page.reload();
  await expect(page.locator('[data-composer] textarea').first()).toBeEditable();
  await page.waitForLoadState('networkidle');
  const held = new Set(sent.filter((one) => one.method === 'GET' && one.failed === undefined).map((one) => one.path + one.query));
  const opened = sent.length;
  await page.getByRole('tab', { name: /Repro/ }).first().click();
  await expect(page.locator('[data-screen="thread"]').filter({ has: page.locator('h1', { hasText: 'Repro' }) }).locator('[data-composer] textarea').first()).toBeEditable();
  await page.waitForLoadState('networkidle');
  const after = sent.slice(opened);
  process.stdout.write(`First switch after a reload: ${JSON.stringify(shown(after))}\n`);
  expect(shown(after.filter((one) => one.method === 'GET' && held.has(one.path + one.query)))).toEqual([]);
  expect(after.filter((one) => /^\/api\/v1\/orgs\/[^/]+\/tree$/.test(one.path) && !one.query.includes('path_to'))).toEqual([]);
});
