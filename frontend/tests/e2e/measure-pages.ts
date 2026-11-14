/** Measures cold loads, repeat loads and real retained-tab returns against an explicitly accepted report.
 *
 * npm run measure:pages -- --base https://orch.example --state signed-in.json --scenario switch
 *   --via /settings/interface --baseline accepted.json --report candidate.json /resources/providers
 *
 * API calls are counted by their starts. Their captured set is allowed to finish after content, so a slow answer
 * cannot disappear from the totals. Incomplete accounting, redirects and native error content fail the command.
 * --runs (default 3), --timeout (default 60000), and regression allowances --allow-calls, --allow-bytes,
 * --allow-ms (default 0) are measurement parameters, never product limits. --ready names visible page content
 * where the native workbench marker is unavailable. --requests lists sanitized paths; credentials are never printed.
 */
import { readFile, writeFile } from 'node:fs/promises';
import { chromium, type Browser, type Page, type Request } from '@playwright/test';
import { compareRuns, pageOutcome, RequestLedger, type MeasuredRequest, type RunNumbers } from './page-measurement.ts';

interface Options {
  base: string; state?: string; cookies: string[]; runs: number; timeout: number; ready?: string;
  scenario: RunNumbers['scenario']; via?: string; baseline?: string; report?: string;
  allowCalls: number; allowBytes: number; allowMs: number; requests: boolean; insecure: boolean; addresses: string[];
}
interface Content { at: number; tab: string | null; outcome: string }
interface Run extends RunNumbers { requests: MeasuredRequest[]; scripts: number; decoded: number }
interface Row extends RunNumbers { address: string; scripts: number; decoded: number; runs: Run[] }
interface Report { version: 1; rows: Row[] }

function parse(argv: readonly string[]): Options {
  const options: Options = { base: '', cookies: [], runs: 3, timeout: 60_000, scenario: 'cold', allowCalls: 0, allowBytes: 0, allowMs: 0, requests: false, insecure: false, addresses: [] };
  for (let at = 0; at < argv.length; at += 1) {
    const arg = argv[at] ?? '';
    const value = () => { const next = argv[++at]; if (next === undefined) throw new Error(`${arg} needs a value`); return next; };
    switch (arg) {
      case '--base': options.base = value(); break;
      case '--state': options.state = value(); break;
      case '--cookie': options.cookies.push(value()); break;
      case '--runs': options.runs = Number(value()); break;
      case '--timeout': options.timeout = Number(value()); break;
      case '--ready': options.ready = value(); break;
      case '--scenario': { const next = value(); if (next !== 'cold' && next !== 'repeat' && next !== 'switch') throw new Error('Unknown scenario'); options.scenario = next; break; }
      case '--via': options.via = value(); break;
      case '--baseline': options.baseline = value(); break;
      case '--report': options.report = value(); break;
      case '--allow-calls': options.allowCalls = Number(value()); break;
      case '--allow-bytes': options.allowBytes = Number(value()); break;
      case '--allow-ms': options.allowMs = Number(value()); break;
      case '--requests': options.requests = true; break;
      case '--insecure': options.insecure = true; break;
      default: if (arg.startsWith('--')) throw new Error(`Unknown option ${arg}`); options.addresses.push(arg);
    }
  }
  if (options.base === '' || options.addresses.length === 0) throw new Error('--base and at least one page address are required');
  if (options.scenario === 'switch' && options.via === undefined) throw new Error('--via names the other page opened before returning to the retained tab');
  if (!Number.isInteger(options.runs) || options.runs < 1 || !Number.isFinite(options.timeout) || options.timeout <= 0) throw new Error('Runs and timeout must be positive');
  for (const allowance of [options.allowCalls, options.allowBytes, options.allowMs]) if (!Number.isFinite(allowance) || allowance < 0) throw new Error('Regression allowances must be nonnegative');
  for (const cookie of options.cookies) if (cookie.indexOf('=') < 1) throw new Error('A cookie needs a name and value');
  return options;
}

/** Waits for visible content of the actual requested address, using the view's own outcome. */
async function content(page: Page, address: string, options: Options, since = 0): Promise<Content> {
  const result = await page.waitForFunction(({ wanted, selector, since }) => {
    const expected = new URL(wanted), current = new URL(location.href);
    const matches = (url: URL) => expected.origin === url.origin && expected.pathname.replace(/\/$/, '') === url.pathname.replace(/\/$/, '')
      && [...expected.searchParams].every(([key, value]) => url.searchParams.get(key) === value);
    if (!matches(current)) return { at: performance.timeOrigin + performance.now(), tab: null, outcome: 'redirect' };
    const visible = (element: Element) => element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden';
    const roots = [...document.querySelectorAll<HTMLElement>('[data-view-instance][data-page-content]')];
    const root = roots.find((element) => visible(element) && matches(new URL(element.dataset.pageAddress ?? '', location.href)));
    if (root?.dataset.pageContent === 'error') return { at: performance.timeOrigin + performance.now(), tab: root.dataset.viewInstance ?? null, outcome: 'error' };
    const ready = selector === undefined ? root?.dataset.pageContent === 'ready' : [...document.querySelectorAll(selector)].some(visible);
    if (!ready) return false;
    const marks = performance.getEntriesByName('orch/page/content').filter((entry) => performance.timeOrigin + entry.startTime >= since);
    const mark = marks.at(-1);
    return { at: performance.timeOrigin + (mark?.startTime ?? performance.now()), tab: root?.dataset.viewInstance ?? null, outcome: 'ready' };
  }, { wanted: address, selector: options.ready, since }, { timeout: options.timeout });
  const answer = await result.jsonValue();
  if (answer === false) throw new Error('The requested content did not become visible');
  await result.dispose();
  const outcome = pageOutcome(address, page.url(), answer.outcome, answer.outcome === 'ready');
  if (outcome !== 'ready') throw new Error(`Requested page produced ${outcome}`);
  return answer;
}

const isCall = (request: MeasuredRequest) => /^\/(?:api|openapi)\//.test(request.path);
const isScript = (request: MeasuredRequest) => request.kind === 'script';
const sanitized = (url: string): string => {
  const address = new URL(url);
  const path = address.pathname.replace(/[0-9a-f]{8}-[0-9a-f-]{27,}/gi, ':id');
  const keys = [...new Set(address.searchParams.keys())].sort();
  return path + (keys.length === 0 ? '' : `?${keys.map((key) => `${encodeURIComponent(key)}=:value`).join('&')}`);
};

async function load(browser: Browser, options: Options, address: string): Promise<Run> {
  const context = await browser.newContext({ ignoreHTTPSErrors: options.insecure, ...(options.state === undefined ? {} : { storageState: options.state }) });
  try {
    await context.addCookies(options.cookies.map((cookie) => ({ name: cookie.slice(0, cookie.indexOf('=')), value: cookie.slice(cookie.indexOf('=') + 1), url: options.base })));
    const page = await context.newPage();
    const ledger = new RequestLedger<Request>();
    const flights = new Map<Request, { start: number; done: Promise<void>; finish: () => void; failed: boolean }>();
    page.on('request', (request) => {
      const start = request.timing().startTime > 0 ? request.timing().startTime : Date.now();
      let finish: () => void = () => undefined;
      const done = new Promise<void>((resolve) => { finish = resolve; });
      flights.set(request, { start, done, finish, failed: false });
      ledger.begin(request, sanitized(request.url()), start, request.resourceType());
    });
    const completed = async (request: Request, failed: boolean) => {
      const flight = flights.get(request);
      if (flight === undefined) return;
      flight.failed ||= failed;
      try {
        const sizes = failed ? null : await request.sizes();
        ledger.finish(request, { end: Date.now(), wire: sizes === null ? null : sizes.responseBodySize + sizes.responseHeadersSize, decoded: null, failed: flight.failed });
      } finally { flight.finish(); }
    };
    page.on('response', (response) => { const flight = flights.get(response.request()); if (flight !== undefined && response.status() >= 400) flight.failed = true; });
    page.on('requestfinished', (request) => { void completed(request, false).catch(() => undefined); });
    page.on('requestfailed', (request) => { void completed(request, true).catch(() => undefined); });
    const wanted = new URL(address, options.base).href;
    let since = Date.now();
    await page.goto(wanted, { waitUntil: 'commit', timeout: options.timeout });
    let shown = await content(page, wanted, options, since);
    if (options.scenario === 'repeat') {
      since = Date.now();
      await page.reload({ waitUntil: 'commit', timeout: options.timeout });
      shown = await content(page, wanted, options, since);
    } else if (options.scenario === 'switch') {
      if (shown.tab === null) throw new Error('A tab-switch measurement requires a native workbench view');
      const tab = shown.tab;
      const away = new URL(options.via ?? '', options.base).href;
      if (away === wanted) throw new Error('--via must name a different page');
      await page.evaluate((href) => { history.pushState({}, '', href); dispatchEvent(new PopStateEvent('popstate')); }, away);
      await content(page, away, { ...options, ready: undefined });
      const target = page.locator(`[role="tab"][id=${JSON.stringify(`tab-${tab}`)}]`);
      since = Date.now();
      await target.click({ timeout: options.timeout });
      shown = await content(page, wanted, options, since);
    }
    const captured = [...flights].filter(([, flight]) => flight.start >= since && flight.start <= shown.at);
    let timeout: ReturnType<typeof setTimeout> | undefined;
    try { await Promise.race([Promise.all(captured.map(([, flight]) => flight.done)), new Promise<void>((resolve) => { timeout = setTimeout(resolve, options.timeout); })]); }
    finally { clearTimeout(timeout); }
    const resources = await page.evaluate(() => (performance.getEntriesByType('resource') as PerformanceResourceTiming[]).map((entry) => ({ url: entry.name, start: performance.timeOrigin + entry.startTime, end: performance.timeOrigin + entry.responseEnd, wire: entry.transferSize, decoded: entry.decodedBodySize })));
    for (const [request, flight] of captured) {
      const matches = resources.filter((resource) => resource.url === request.url()).sort((a, b) => Math.abs(a.start - flight.start) - Math.abs(b.start - flight.start));
      const timing = matches[0];
      if (timing !== undefined) resources.splice(resources.indexOf(timing), 1);
      if (timing !== undefined) ledger.finish(request, { ...timing, failed: flight.failed });
    }
    const before = ledger.before(shown.at, since);
    if (before.some((request) => request.end === null || request.wire === null || request.failed)) throw new Error(`Incomplete or failed request accounting (${before.filter((request) => request.end === null || request.wire === null || request.failed).length} captured requests)`);
    const calls = before.filter(isCall);
    if (calls.some((request) => request.decoded === null)) throw new Error('Decoded API response sizes are unavailable; this run cannot be accepted');
    return { scenario: options.scenario, calls: calls.length, bytes: before.reduce((total, request) => total + (request.wire ?? 0), 0), decoded: calls.reduce((total, request) => total + (request.decoded ?? 0), 0), scripts: before.filter(isScript).length, content: shown.at - since, requests: before.map((request) => ({ ...request, start: request.start - since, end: request.end === null ? null : request.end - since })) };
  } finally { await context.close(); }
}

const median = (values: number[]) => values.sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? Number.NaN;
const say = (line: string) => { process.stdout.write(`${line}\n`); };
async function main(): Promise<void> {
  const options = parse(process.argv.slice(2));
  const accepted = options.baseline === undefined ? undefined : JSON.parse(await readFile(options.baseline, 'utf8')) as Report;
  if (accepted !== undefined && (accepted.version !== 1 || !Array.isArray(accepted.rows))) throw new Error('The accepted report has an unknown shape');
  const report: Report = { version: 1, rows: [] };
  const browser = await chromium.launch();
  try {
    for (const address of options.addresses) {
      const runs: Run[] = [];
      for (let run = 0; run < options.runs; run += 1) runs.push(await load(browser, options, address));
      const middle = (pick: (run: Run) => number) => median(runs.map(pick));
      report.rows.push({ address, scenario: options.scenario, runs, calls: middle((run) => run.calls), bytes: middle((run) => run.bytes), decoded: middle((run) => run.decoded), scripts: middle((run) => run.scripts), content: middle((run) => run.content) });
    }
  } finally { await browser.close(); }
  if (options.report !== undefined) await writeFile(options.report, JSON.stringify(report, null, 2) + '\n');
  say('| Page | Scenario | Calls before content | Wire bytes | Decoded API bytes | Scripts | Content ms | Change in calls / bytes / ms |');
  say('|---|---|---|---|---|---|---|---|');
  let failed = false;
  for (const row of report.rows) {
    const previous = accepted?.rows.find((candidate) => candidate.address === row.address && candidate.scenario === row.scenario);
    if (accepted !== undefined && previous === undefined) throw new Error('The accepted report has no matching page and scenario');
    const delta = previous === undefined ? undefined : compareRuns(row, previous);
    say(`| ${sanitized(new URL(row.address, options.base).href)} | ${row.scenario} | ${row.calls} | ${row.bytes} | ${row.decoded} | ${row.scripts} | ${Math.round(row.content)} | ${delta === undefined ? 'uncompared' : `${delta.calls} / ${delta.bytes} / ${Math.round(delta.content)}`} |`);
    if (row.scenario === 'switch' && row.calls !== 0 || delta !== undefined && (delta.calls > options.allowCalls || delta.bytes > options.allowBytes || delta.content > options.allowMs)) failed = true;
    if (options.requests) for (const request of row.runs[0]?.requests ?? []) say(`${Math.round(request.start)} ms  ${request.wire ?? '?'} bytes  ${request.path}`);
  }
  if (failed) throw new Error('The retained-tab pass line or accepted-run regression allowance was exceeded');
}
void main().catch((error: unknown) => { process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; });
