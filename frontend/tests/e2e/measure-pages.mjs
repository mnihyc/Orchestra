#!/usr/bin/env node
/** Loads a list of addresses on a running installation and says, for each page, what was asked for before its content, how much that
 * was, and when the content stood on the screen. It prints one table to paste, and judges nothing: no figure here is a limit.
 *
 *   node tests/e2e/measure-pages.mjs --base https://orch.example --state signed-in.json /thread/<id> /resources/providers /org/acme
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
import { chromium } from '@playwright/test';

function parse(argv) {
  const options = { runs: 3, timeout: 60_000, cookies: [], addresses: [], warm: false, requests: false, insecure: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const value = () => { i += 1; if (argv[i] === undefined) throw new Error(`${arg} needs a value`); return argv[i]; };
    if (arg === '--base') options.base = value();
    else if (arg === '--state') options.state = value();
    else if (arg === '--cookie') options.cookies.push(value());
    else if (arg === '--runs') options.runs = Number(value());
    else if (arg === '--ready') options.ready = value();
    else if (arg === '--timeout') options.timeout = Number(value());
    else if (arg === '--warm') options.warm = true;
    else if (arg === '--requests') options.requests = true;
    else if (arg === '--insecure') options.insecure = true;
    else if (arg.startsWith('--')) throw new Error(`Unknown option ${arg}`);
    else options.addresses.push(arg);
  }
  if (options.base === undefined) throw new Error('--base is needed: the installation to load the pages from');
  if (options.addresses.length === 0) throw new Error('Name at least one address, such as /thread/<id>');
  if (!Number.isInteger(options.runs) || options.runs < 1) throw new Error('--runs is a whole number of one or more');
  return options;
}

/** Runs in the page before anything else: notes when its content has stood on the screen. */
function watchContent({ ready }) {
  const check = () => {
    if (window.__orchContentAt !== undefined) return;
    let there;
    if (ready) there = document.querySelector(ready) !== null;
    else {
      const main = document.querySelector('main, [role="main"]');
      there = main !== null && main.querySelector('[aria-busy="true"]') === null && (main.textContent ?? '').trim().length > 40;
    }
    if (there) window.__orchContentAt = performance.now();
  };
  let scheduled = false;
  const later = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; check(); });
  };
  new MutationObserver(later).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  document.addEventListener('DOMContentLoaded', later);
}

/** Runs in the page when its content has stood: what it asked for and what it marked. */
function collect() {
  const marks = performance.getEntriesByType('mark').filter((entry) => entry.name.startsWith('orch/')).map((entry) => ({ name: entry.name, at: entry.startTime, detail: entry.detail }));
  const resources = performance.getEntriesByType('resource').map((entry) => ({
    path: new URL(entry.name).pathname + new URL(entry.name).search, kind: entry.initiatorType,
    start: entry.startTime, end: entry.responseEnd, wire: entry.transferSize, decoded: entry.decodedBodySize,
  }));
  return { marks, resources, contentAt: window.__orchContentAt };
}

async function load(browser, options, address, measure) {
  const context = await browser.newContext({ ignoreHTTPSErrors: options.insecure, ...(options.state === undefined ? {} : { storageState: options.state }) });
  try {
    if (options.cookies.length > 0) {
      const url = new URL(options.base);
      await context.addCookies(options.cookies.map((cookie) => {
        const at = cookie.indexOf('=');
        return { name: cookie.slice(0, at), value: cookie.slice(at + 1), domain: url.hostname, path: '/' };
      }));
    }
    const visit = async () => {
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
  } finally { await context.close(); }
}

const isCall = (resource) => resource.path.startsWith('/api/') || resource.path.startsWith('/openapi/');
const isScript = (resource) => resource.kind === 'script' || /\.m?js(\?|$)/.test(resource.path);
const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length === 0 ? Number.NaN : sorted[Math.floor((sorted.length - 1) / 2)];
};
const kb = (bytes) => (Number.isNaN(bytes) ? '-' : (bytes / 1024).toFixed(1));
const ms = (value) => (value === undefined || Number.isNaN(value) ? '-' : String(Math.round(value)));

function summarize(result) {
  const before = result.resources.filter((resource) => resource.start <= result.contentAt);
  const calls = before.filter(isCall);
  const scripts = before.filter(isScript);
  const mark = (name) => result.marks.find((entry) => entry.name === `orch/${name}`)?.at;
  return {
    calls: calls.length, callDecoded: calls.reduce((sum, call) => sum + call.decoded, 0), callWire: calls.reduce((sum, call) => sum + call.wire, 0),
    scripts: scripts.length, scriptWire: scripts.reduce((sum, script) => sum + script.wire, 0),
    shellDrawn: mark('start/shell-drawn'), content: result.contentAt, list: calls,
  };
}

async function main() {
  const options = parse(process.argv.slice(2));
  const browser = await chromium.launch({ args: ['--disable-gpu'] });
  const rows = [];
  try {
    for (const address of options.addresses) {
      const runs = [];
      for (let run = 0; run < options.runs; run += 1) runs.push(summarize(await load(browser, options, address)));
      const of = (pick) => median(runs.map(pick));
      rows.push({ address, runs,
        calls: of((r) => r.calls), callDecoded: of((r) => r.callDecoded), callWire: of((r) => r.callWire),
        scripts: of((r) => r.scripts), scriptWire: of((r) => r.scriptWire), shellDrawn: of((r) => r.shellDrawn ?? Number.NaN), content: of((r) => r.content) });
    }
  } finally { await browser.close(); }
  const visit = options.warm ? 'repeat visit' : 'cold load';
  console.log(`${visit}, middle of ${options.runs} load${options.runs === 1 ? '' : 's'}, ${options.base}\n`);
  console.log('| Page | Calls before content | Their bytes (uncompressed, KB) | Their bytes (on the wire, KB) | Scripts before content | Their bytes (on the wire, KB) | Frame drawn (ms) | Content (ms) |');
  console.log('|---|---|---|---|---|---|---|---|');
  for (const row of rows) {
    console.log(`| ${row.address} | ${row.calls} | ${kb(row.callDecoded)} | ${kb(row.callWire)} | ${row.scripts} | ${kb(row.scriptWire)} | ${ms(row.shellDrawn)} | ${ms(row.content)} |`);
  }
  if (options.requests) {
    for (const row of rows) {
      console.log(`\n${row.address}: the calls made before the content (first load), in the order they began`);
      console.log('| Began (ms) | Ended (ms) | Uncompressed (KB) | On the wire (KB) | Request |');
      console.log('|---|---|---|---|---|');
      for (const call of [...(row.runs[0]?.list ?? [])].sort((a, b) => a.start - b.start)) {
        console.log(`| ${ms(call.start)} | ${ms(call.end)} | ${kb(call.decoded)} | ${kb(call.wire)} | ${call.path} |`);
      }
    }
  }
}

main().catch((error) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1; });
