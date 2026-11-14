/** Loads a list of addresses on a running installation and says, for each page, what was asked for before its content, how much that
 * was, and when the content stood on the screen. It prints one table to paste, and judges nothing: no figure here is a limit.
 *
 *   node tests/e2e/measure-pages.ts --base https://orch.example --state signed-in.json /thread/<id> /resources/providers /org/acme
 *
 * Options
 *   --base URL        the installation (required)
 *   --state FILE      a Playwright storage state of somebody signed in (`context.storageState({ path })`); or
 *   --cookie NAME=VALUE   a cookie to send, as many times as needed (the session cookie of a browser that is signed in)
 *   --runs N          loads of each address, each in a browser of its own with nothing kept (default 3); the table shows the middle
 *   --warm            measure a repeat visit: the page is loaded once first, and the load that is measured follows it in the same browser
 *   --ready SELECTOR  what must be on the screen for the content to be there (default: the main area holds some text and nothing in
 *                     it is marked busy); a selector of the page's own content is exact where this is not
 *   --requests        also list each request made before the content, for the first load of each address
 *   --insecure        accept a certificate that is not trusted
 *   --timeout MS      how long to wait for the content (default 60000)
 *
 * What it reads is the browser's own: the resource timing of the page, and the marks the page makes under the prefix `orch/`
 * (`src/lib/marks.ts`). A request is one of the page's calls to the API (`/api/`, `/openapi/`) that began before the content
 * stood; its bytes are what was said at the end of its answer (uncompressed) and what went over the wire.
 */
import { type Browser, chromium } from '@playwright/test';

declare global {
  interface Window {
    /** When the content first stood on the screen, on the page's own clock; set by `watchContent`. */
    __orchContentAt?: number;
  }
}

interface Options {
  base: string;
  state: string | undefined;
  cookies: string[];
  runs: number;
  ready: string | undefined;
  timeout: number;
  warm: boolean;
  requests: boolean;
  insecure: boolean;
  addresses: string[];
}

/** What the page says of itself once its content has stood. */
interface Collected {
  marks: { name: string; at: number }[];
  resources: { path: string; kind: string; start: number; end: number; wire: number; decoded: number }[];
  contentAt: number | undefined;
}

interface Summary {
  calls: number;
  callDecoded: number;
  callWire: number;
  scripts: number;
  scriptWire: number;
  shellDrawn: number | undefined;
  content: number;
  list: Collected['resources'];
}

function parse(argv: readonly string[]): Options {
  let base: string | undefined;
  let state: string | undefined;
  let ready: string | undefined;
  let runs = 3;
  let timeout = 60_000;
  let warm = false;
  let requests = false;
  let insecure = false;
  const cookies: string[] = [];
  const addresses: string[] = [];
  for (let at = 0; at < argv.length; at += 1) {
    const arg = argv[at] ?? '';
    const value = (): string => {
      at += 1;
      const next = argv[at];
      if (next === undefined) throw new Error(`${arg} needs a value`);
      return next;
    };
    switch (arg) {
      case '--base': base = value(); break;
      case '--state': state = value(); break;
      case '--cookie': cookies.push(value()); break;
      case '--runs': runs = Number(value()); break;
      case '--ready': ready = value(); break;
      case '--timeout': timeout = Number(value()); break;
      case '--warm': warm = true; break;
      case '--requests': requests = true; break;
      case '--insecure': insecure = true; break;
      default:
        if (arg.startsWith('--')) throw new Error(`Unknown option ${arg}`);
        addresses.push(arg);
    }
  }
  if (base === undefined) throw new Error('--base is needed: the installation to load the pages from');
  if (addresses.length === 0) throw new Error('Name at least one address, such as /thread/<id>');
  if (!Number.isInteger(runs) || runs < 1) throw new Error('--runs is a whole number of one or more');
  return { base, state, cookies, runs, ready, timeout, warm, requests, insecure, addresses };
}

/** Runs in the page before anything else: notes when its content has stood on the screen. */
function watchContent({ ready }: { ready: string | undefined }): void {
  const check = (): void => {
    if (window.__orchContentAt !== undefined) return;
    let there: boolean;
    if (ready !== undefined) there = document.querySelector(ready) !== null;
    else {
      const main = document.querySelector('main, [role="main"]');
      there = main !== null && main.querySelector('[aria-busy="true"]') === null && main.textContent.trim().length > 40;
    }
    if (there) window.__orchContentAt = performance.now();
  };
  let scheduled = false;
  const later = (): void => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; check(); });
  };
  new MutationObserver(later).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  document.addEventListener('DOMContentLoaded', later);
}

/** Runs in the page when its content has stood: what it asked for and what it marked. */
function collect(): Collected {
  const marks = performance.getEntriesByType('mark').filter((entry) => entry.name.startsWith('orch/')).map((entry) => ({ name: entry.name, at: entry.startTime }));
  const resources = (performance.getEntriesByType('resource') as PerformanceResourceTiming[]).map((entry) => {
    const address = new URL(entry.name);
    return { path: address.pathname + address.search, kind: entry.initiatorType, start: entry.startTime, end: entry.responseEnd, wire: entry.transferSize, decoded: entry.decodedBodySize };
  });
  return { marks, resources, contentAt: window.__orchContentAt };
}

async function load(browser: Browser, options: Options, address: string): Promise<Collected> {
  const context = await browser.newContext({ ignoreHTTPSErrors: options.insecure, ...(options.state === undefined ? {} : { storageState: options.state }) });
  try {
    if (options.cookies.length > 0) {
      const url = new URL(options.base);
      await context.addCookies(options.cookies.map((cookie) => {
        const at = cookie.indexOf('=');
        return { name: cookie.slice(0, at), value: cookie.slice(at + 1), domain: url.hostname, path: '/' };
      }));
    }
    const visit = async (): Promise<Collected> => {
      const page = await context.newPage();
      await page.addInitScript(watchContent, { ready: options.ready });
      await page.goto(new URL(address, options.base).href, { waitUntil: 'commit' });
      await page.waitForFunction(() => window.__orchContentAt !== undefined, null, { timeout: options.timeout });
      const result = await page.evaluate(collect);
      await page.close();
      return result;
    };
    if (options.warm) await visit();
    return await visit();
  } finally {
    await context.close();
  }
}

type Resource = Collected['resources'][number];
const isCall = (resource: Resource): boolean => resource.path.startsWith('/api/') || resource.path.startsWith('/openapi/');
const isScript = (resource: Resource): boolean => resource.kind === 'script' || /\.m?js(\?|$)/.test(resource.path);
const sum = (resources: readonly Resource[], pick: (resource: Resource) => number): number => resources.reduce((total, resource) => total + pick(resource), 0);

function median(values: readonly number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length === 0 ? Number.NaN : (sorted[Math.floor((sorted.length - 1) / 2)] ?? Number.NaN);
}
const kb = (bytes: number): string => (Number.isNaN(bytes) ? '-' : (bytes / 1024).toFixed(1));
const ms = (value: number | undefined): string => (value === undefined || Number.isNaN(value) ? '-' : String(Math.round(value)));
const say = (line: string): void => { process.stdout.write(`${line}\n`); };

function summarize(result: Collected): Summary {
  const content = result.contentAt ?? Number.NaN;
  const before = result.resources.filter((resource) => resource.start <= content);
  const calls = before.filter(isCall);
  const scripts = before.filter(isScript);
  return {
    calls: calls.length, callDecoded: sum(calls, (call) => call.decoded), callWire: sum(calls, (call) => call.wire),
    scripts: scripts.length, scriptWire: sum(scripts, (script) => script.wire),
    shellDrawn: result.marks.find((entry) => entry.name === 'orch/start/shell-drawn')?.at, content, list: calls,
  };
}

async function main(): Promise<void> {
  const options = parse(process.argv.slice(2));
  const browser = await chromium.launch({ args: ['--disable-gpu'] });
  const rows: { address: string; runs: Summary[]; calls: number; callDecoded: number; callWire: number; scripts: number; scriptWire: number; shellDrawn: number; content: number }[] = [];
  try {
    for (const address of options.addresses) {
      const runs: Summary[] = [];
      for (let run = 0; run < options.runs; run += 1) runs.push(summarize(await load(browser, options, address)));
      const of = (pick: (summary: Summary) => number): number => median(runs.map(pick));
      rows.push({
        address, runs,
        calls: of((r) => r.calls), callDecoded: of((r) => r.callDecoded), callWire: of((r) => r.callWire),
        scripts: of((r) => r.scripts), scriptWire: of((r) => r.scriptWire), shellDrawn: of((r) => r.shellDrawn ?? Number.NaN), content: of((r) => r.content),
      });
    }
  } finally {
    await browser.close();
  }
  say(`${options.warm ? 'repeat visit' : 'cold load'}, middle of ${options.runs} load${options.runs === 1 ? '' : 's'}, ${options.base}\n`);
  say('| Page | Calls before content | Their bytes (uncompressed, KB) | Their bytes (on the wire, KB) | Scripts before content | Their bytes (on the wire, KB) | Frame drawn (ms) | Content (ms) |');
  say('|---|---|---|---|---|---|---|---|');
  for (const row of rows) {
    say(`| ${row.address} | ${row.calls} | ${kb(row.callDecoded)} | ${kb(row.callWire)} | ${row.scripts} | ${kb(row.scriptWire)} | ${ms(row.shellDrawn)} | ${ms(row.content)} |`);
  }
  if (!options.requests) return;
  for (const row of rows) {
    say(`\n${row.address}: the calls made before the content (first load), in the order they began`);
    say('| Began (ms) | Ended (ms) | Uncompressed (KB) | On the wire (KB) | Request |');
    say('|---|---|---|---|---|');
    for (const call of [...(row.runs[0]?.list ?? [])].sort((a, b) => a.start - b.start)) {
      say(`| ${ms(call.start)} | ${ms(call.end)} | ${kb(call.decoded)} | ${kb(call.wire)} | ${call.path} |`);
    }
  }
}

void main().catch((error: unknown) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
