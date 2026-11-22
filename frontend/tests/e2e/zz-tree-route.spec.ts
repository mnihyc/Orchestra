/** Throwaway experiment: what ends a tree read that the browser reports canceled. */
import { expect, type Page, test } from '@playwright/test';

import { scenario, signIn } from './support';

const ROUND_TRIP_MS = 200;

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

/** Every abort, every stream cancel and every tree fetch of the page, with the code that asked. */
function instrument(polyfillAny: boolean): void {
  const log: string[] = [];
  Object.defineProperty(window, '__treeLog', { value: log });
  const now = () => Date.now();
  const stack = () => (new Error().stack ?? '').split('\n').slice(2, 10).map((line) => line.trim().replace(/https?:\/\/[^/]+\/assets\//, '')).join(' | ');
  const abort = AbortController.prototype.abort;
  AbortController.prototype.abort = function (this: AbortController, reason?: unknown) {
    if (!this.signal.aborted) log.push(`${now()} AbortController.abort ${String(reason)} ${stack()}`);
    return abort.call(this, reason);
  };
  const readerCancel = ReadableStreamDefaultReader.prototype.cancel;
  ReadableStreamDefaultReader.prototype.cancel = function (this: ReadableStreamDefaultReader, reason?: unknown) {
    log.push(`${now()} reader.cancel ${String(reason)} ${stack()}`);
    return readerCancel.call(this, reason);
  };
  const streamCancel = ReadableStream.prototype.cancel;
  ReadableStream.prototype.cancel = function (this: ReadableStream, reason?: unknown) {
    log.push(`${now()} stream.cancel ${String(reason)} ${stack()}`);
    return streamCancel.call(this, reason);
  };
  if (polyfillAny) {
    AbortSignal.any = (signals: Iterable<AbortSignal>) => {
      const joined = new AbortController();
      for (const signal of signals) {
        if (signal.aborted) { joined.abort(signal.reason); break; }
        signal.addEventListener('abort', () => joined.abort(signal.reason), { once: true });
      }
      return joined.signal;
    };
  }
  const original = window.fetch;
  window.fetch = function (input: RequestInfo | URL, init?: RequestInit) {
    const url = input instanceof Request ? input.url : String(input);
    const tree = /\/tree\?/.test(url);
    const tag = url.slice(-12);
    if (tree) {
      const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined);
      log.push(`${now()} fetch ${tag} signal=${signal === undefined || signal === null ? 'none' : signal.aborted ? 'aborted' : 'live'}`);
      signal?.addEventListener('abort', () => log.push(`${now()} signal-abort ${tag} ${String(signal.reason)}`));
    }
    const answer = original.call(this, input, init);
    if (tree) {
      answer.then((response) => log.push(`${now()} response ${tag} ${response.status} cl=${response.headers.get('content-length')} te=${response.headers.get('transfer-encoding')}`),
        (error: unknown) => log.push(`${now()} rejected ${tag} ${error instanceof Error ? `${error.name} ${error.message}` : String(error)}`));
    }
    return answer;
  };
}

test.beforeEach(async () => { await scenario(); });

const MODES = [
  ...Array.from({ length: 3 }, () => ({ name: 'direct', routed: false, polyfillAny: false, latency: true })),
  ...Array.from({ length: 3 }, () => ({ name: 'direct-any-polyfill', routed: false, polyfillAny: true, latency: true })),
  ...Array.from({ length: 3 }, () => ({ name: 'direct-no-latency', routed: false, polyfillAny: false, latency: false })),
  ...Array.from({ length: 8 }, () => ({ name: 'routed', routed: true, polyfillAny: false, latency: true })),
];

MODES.forEach(({ name, routed, polyfillAny, latency }, index) => {
  test(`tree reads ${index} ${name}`, async ({ page, browser }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    if (routed) await nativeTreeReplies(page);
    await page.addInitScript(instrument, polyfillAny);
    await signIn(page);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Network.enable');
    const events: string[] = [];
    const tags = new Map<string, string>();
    const failedApi: string[] = [];
    cdp.on('Network.requestWillBeSent', ({ requestId, request }) => {
      if (/\/tree\?/.test(request.url)) { tags.set(requestId, request.url.slice(-12)); events.push(`${Date.now()} cdp-sent ${request.url.slice(-12)} prio=${request.initialPriority}`); }
    });
    cdp.on('Network.responseReceived', ({ requestId, response }) => {
      const tag = tags.get(requestId);
      if (tag !== undefined) events.push(`${Date.now()} cdp-response ${tag} ${response.status} proto=${response.protocol ?? ''} cl=${String(response.headers['content-length'] ?? response.headers['Content-Length'] ?? '')} sw=${String(response.fromServiceWorker)} disk=${String(response.fromDiskCache)}`);
    });
    const received = new Map<string, number>();
    cdp.on('Network.dataReceived', ({ requestId, dataLength }) => {
      const tag = tags.get(requestId);
      if (tag !== undefined) received.set(requestId, (received.get(requestId) ?? 0) + dataLength);
    });
    cdp.on('Network.loadingFinished', ({ requestId, encodedDataLength }) => {
      const tag = tags.get(requestId);
      if (tag !== undefined) events.push(`${Date.now()} cdp-finished ${tag} data=${received.get(requestId) ?? 0} encoded=${encodedDataLength}`);
    });
    cdp.on('Network.loadingFailed', ({ requestId, errorText, canceled, blockedReason }) => {
      const tag = tags.get(requestId);
      if (tag !== undefined) events.push(`${Date.now()} cdp-failed ${tag} ${errorText} canceled=${String(canceled)} blocked=${String(blockedReason)} data=${received.get(requestId) ?? 0}`);
      else failedApi.push(`${Date.now()} other-failed ${requestId} ${errorText}`);
    });
    if (latency) await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: ROUND_TRIP_MS, downloadThroughput: -1, uploadThroughput: -1 });
    const opened = Date.now();
    await page.goto('/resources/providers');
    await page.waitForFunction(() => performance.getEntriesByName('orch/page/content', 'mark').some((entry) => ((entry as PerformanceMark).detail as { outcome?: string } | null)?.outcome === 'ready'), undefined, { timeout: 60_000 });
    await page.waitForLoadState('networkidle');
    const pageLog = await page.evaluate(() => (window as Window & { __treeLog?: string[] }).__treeLog ?? []);
    const canceled = events.filter((line) => line.includes('cdp-failed'));
    process.stdout.write(`TREE ${index} ${name} chrome=${browser.version()} sent=${events.filter((line) => line.includes('cdp-sent')).length} canceled=${canceled.length}\n`);
    if (canceled.length > 0 || index === 0 || index === 6) {
      const timeline = [...events, ...pageLog, ...failedApi].map((line) => {
        const at = Number(line.slice(0, line.indexOf(' ')));
        return { at, line: `${at - opened} ${line.slice(line.indexOf(' ') + 1)}` };
      }).sort((a, b) => a.at - b.at).map((entry) => entry.line);
      process.stdout.write(`TIMELINE ${index} ${name} ${JSON.stringify(timeline)}\n`);
    }
    expect(true).toBe(true);
  });
});
