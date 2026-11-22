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

interface Sent { method: string; path: string; query: string; wall: number; failed?: string; by?: string }
interface Fetched { path: string; wall: number }

/** Every request of the page from now on, by the browser's own clock, with what ended it early. */
async function watch(page: Page): Promise<{ cdp: CDPSession; sent: Sent[]; files: Fetched[] }> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  const sent: Sent[] = [], files: Fetched[] = [];
  const byId = new Map<string, Sent>();
  cdp.on('Network.requestWillBeSent', ({ requestId, request, wallTime, initiator }) => {
    const url = new URL(request.url);
    if (!url.pathname.startsWith('/api/')) { files.push({ path: url.pathname, wall: wallTime * 1000 }); return; }
    if (!url.pathname.startsWith('/api/v1/') || url.pathname === '/api/v1/ws') return;
    // Which code asked: the first frames of the asking stack, by file and position, for a read that is asked twice.
    const by = (initiator.stack?.callFrames ?? []).slice(0, 4).map((frame) => `${frame.functionName || '?'}@${frame.url.split('/').at(-1) ?? ''}:${frame.lineNumber}:${frame.columnNumber}`).join(' < ');
    const one = { method: request.method, path: url.pathname, query: url.search, wall: wallTime * 1000, by };
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


test.beforeEach(async () => { await scenario(); });

for (const intercepted of [true, false]) {
  test(`tree reads ${intercepted ? 'through' : 'without'} the route interception`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    const routed: string[] = [];
    if (intercepted) {
      await page.route(/\/api\/v1\/orgs\/[^/]+\/tree\?/, async (route) => {
        const url = new URL(route.request().url());
        const started = Date.now();
        routed.push(`${started} start ${url.search.slice(-12)}`);
        let response;
        try { response = await route.fetch(); } catch (error) { routed.push(`${Date.now()} fetch-failed ${url.search.slice(-12)} ${String(error)}`); throw error; }
        routed.push(`${Date.now()} fetched ${url.search.slice(-12)} ${response.status()}`);
        if (!response.ok() || !['parent_id', 'path_to', 'sort', 'q', 'kind', 'below_id'].some((name) => url.searchParams.has(name))) {
          await route.fulfill({ response }); routed.push(`${Date.now()} fulfilled-raw ${url.search.slice(-12)}`); return;
        }
        const json = await response.json() as Record<string, unknown>;
        delete json.settings;
        try { await route.fulfill({ response, json }); routed.push(`${Date.now()} fulfilled ${url.search.slice(-12)}`); }
        catch (error) { routed.push(`${Date.now()} fulfill-failed ${url.search.slice(-12)} ${String(error)}`); }
      });
    }
    page.on('requestfailed', (request) => { if (/\/tree\?/.test(request.url())) routed.push(`${Date.now()} requestfailed ${new URL(request.url()).search.slice(-12)} ${request.failure()?.errorText ?? ''}`); });
    await signIn(page);
    const { cdp, sent } = await watch(page);
    await longLine(cdp);
    await page.goto('/resources/providers');
    await contentShown(page, /^\/resources\/providers$/);
    await page.waitForLoadState('networkidle');
    const tree = sent.filter((one) => /^\/api\/v1\/orgs\/[^/]+\/tree$/.test(one.path));
    const failed = tree.filter((one) => one.failed !== undefined);
    process.stdout.write(`TREE ${intercepted ? 'routed' : 'direct'} reads=${tree.length} canceled=${failed.length} ${failed.map((one) => one.query.slice(-12)).join(',')}\n`);
    if (failed.length > 0) process.stdout.write(`ROUTE ${JSON.stringify(routed)}\n`);
  });
}
