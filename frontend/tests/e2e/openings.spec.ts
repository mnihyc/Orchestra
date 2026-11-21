/** The first page a person opens shows its content within a second at a 200 ms round trip, from one request that restores
 * the session: Home, the resource lists and Settings like a thread. Timed in the page itself against its own content mark. */
import { expect, type CDPSession, type Page, test } from '@playwright/test';

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

const shown = (requests: readonly Sent[]) => requests.map((one) => `${one.method} ${one.path}${one.query}${one.failed === undefined ? '' : ` (${one.failed})`}`);

const PAGES: { name: string; address: string; content: RegExp | null }[] = [
  { name: 'Home', address: '/', content: null },
  { name: 'Providers', address: '/resources/providers', content: /^\/resources\/providers$/ },
  { name: 'Machines', address: '/resources/machines', content: /^\/resources\/machines$/ },
  { name: 'Settings', address: '/settings', content: /^\/settings(?:\/|$)/ },
];

test.beforeEach(async () => { await scenario(); });

for (const { name, address, content } of PAGES) {
  test(`${name} opens again within a second at 200 ms from one request that restores the session`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await signIn(page);
    // The person was here before: the device has the page's code and knows what the page reads first.
    await page.goto(address);
    await contentShown(page, content);
    await page.waitForLoadState('networkidle');
    const { cdp, sent } = await watch(page);
    await longLine(cdp);
    await page.reload({ waitUntil: 'commit' });
    const drawn = await contentShown(page, content);
    const before = sent.filter((one) => one.wall <= drawn.wall);
    process.stdout.write(`${name} warm at ${ROUND_TRIP_MS} ms: content ${Math.round(drawn.ms)} ms at ${drawn.address}; requests before it: ${JSON.stringify(shown(before))}\n`);
    expect(shown(before)).toEqual([expect.stringMatching(/^POST \/api\/v1\/auth\/session\/open\?read=%2Fme%2Fbootstrap/)]);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  });

  test(`${name} opens within a second at 200 ms with nothing of its code in the browser's cache`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await signIn(page);
    await page.goto(address);
    await contentShown(page, content);
    await page.waitForLoadState('networkidle');
    const { cdp, sent, files } = await watch(page);
    await cdp.send('Network.clearBrowserCache');
    await longLine(cdp);
    await page.goto(address, { waitUntil: 'commit' });
    const drawn = await contentShown(page, content);
    const before = sent.filter((one) => one.wall <= drawn.wall);
    process.stdout.write(`${name} cold at ${ROUND_TRIP_MS} ms: content ${Math.round(drawn.ms)} ms at ${drawn.address}; requests before it: ${JSON.stringify(shown(before))}; files: ${JSON.stringify(waves(files, drawn.wall))}\n`);
    expect(shown(before)).toEqual([expect.stringMatching(/^POST \/api\/v1\/auth\/session\/open\?read=%2Fme%2Fbootstrap/)]);
    expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
  });
}

test('the sign-in page shows its form within a second at 200 ms with nothing in the browser cache', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const { cdp, sent, files } = await watch(page);
  await cdp.send('Network.clearBrowserCache');
  await longLine(cdp);
  await page.goto('/login', { waitUntil: 'commit' });
  const form = await page.waitForFunction(() => {
    const user = document.querySelector<HTMLInputElement>('input[autocomplete=username]');
    const password = document.querySelector<HTMLInputElement>('input[autocomplete=current-password]');
    if (!user?.getBoundingClientRect().height || user.disabled || !password?.getBoundingClientRect().height || password.disabled) return false;
    return { ms: performance.now(), wall: performance.timeOrigin + performance.now() };
  }, undefined, { polling: 'raf', timeout: 60_000 });
  const drawn = await form.jsonValue() as { ms: number; wall: number };
  process.stdout.write(`Sign-in form cold at ${ROUND_TRIP_MS} ms: ${Math.round(drawn.ms)} ms; requests before it: ${JSON.stringify(shown(sent.filter((one) => one.wall <= drawn.wall)))}; files: ${JSON.stringify(waves(files, drawn.wall))}\n`);
  expect(drawn.ms).toBeLessThanOrEqual(BAR_MS);
});

test('signing in shows the saved conversation within a second of the login answer at 200 ms', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
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
});

test('a first opening reads each part of the navigation tree once and lets none of them go', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await nativeTreeReplies(page);
  await signIn(page);
  const { cdp, sent } = await watch(page);
  await longLine(cdp);
  await page.goto('/resources/providers');
  await contentShown(page, /^\/resources\/providers$/);
  await page.waitForLoadState('networkidle');
  const tree = sent.filter((one) => /^\/api\/v1\/orgs\/[^/]+\/tree$/.test(one.path));
  process.stdout.write(`Tree reads of a first opening: ${JSON.stringify(shown(tree))}\n`);
  expect(tree.length).toBeGreaterThan(0);
  expect(tree.filter((one) => one.failed !== undefined)).toEqual([]);
  expect(new Set(tree.map((one) => one.path + one.query)).size).toBe(tree.length);
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
