/** The capture tool's gates use local browser pages, never a real review login or a reset/constructed world. */
import { expect, test, type Page, type Request } from '@playwright/test';
import { EventEmitter } from 'node:events';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { ACME } from '../tests/doubles/fixtures.ts';
import { SIGNED_IN_ROUTES, SIGNED_OUT_ROUTES } from './layout-routes.ts';
import { ACTORS, IDLE_MS, parseConfig, mapRoute, THEMES, VIEWPORTS, type CaptureConfig } from './capture/config.ts';
import { captureManifest, TARGET_SEEDS, THREAD_CONTAINERS, THREAD_VIEWS } from './capture/manifest.ts';
import { caption } from './capture/open.ts';
import { freshStorage } from './capture/session.ts';
import { contentReady } from './capture/ready.ts';
import { bounded } from './capture/deadline.ts';
import { runTimeout } from './capture/run.ts';
import { PageRequests, requestReceipt, safeEndpoint, type RequestRow } from './capture/requests.ts';
import { contactHTML, writeSheets, type CaptureRun, type ScreenReceipt } from './capture/artifacts.ts';

const input = { baseURL: 'http://127.0.0.1:41800', people: { ada: { passwordEnv: 'CAPTURE_TEST_ADA' }, vera: { passwordEnv: 'CAPTURE_TEST_VERA' }, ben: { passwordEnv: 'CAPTURE_TEST_BEN' } } };
const environment: NodeJS.ProcessEnv = { NODE_ENV: 'test', CAPTURE_TEST_ADA: 'test-only-a', CAPTURE_TEST_VERA: 'test-only-v', CAPTURE_TEST_BEN: 'test-only-b' };
const config = (): CaptureConfig => parseConfig(input, environment);
function screen(number: number, theme: 'light' | 'dark' = 'light'): ScreenReceipt {
  return { number, state: `screen-${number}`, label: `Screen ${number}`, pattern: '/thread/$threadId', route: '/thread/:id', landed: '/thread/:id', actor: 'ada', as_admin: false, observed_as_admin: null,
    viewport: { width: 1440, height: 900 }, theme, locale: 'en', kind: 'route', ready: true, issues: [], screenshot: null, requests: requestReceipt([], [], IDLE_MS) };
}

test('a retained world reuses every inventory entry and its explicit open states without a copy of the route list', () => {
  const held = config();
  held.targets = { org: '/org/review-team', project: '/project/real-project', thread: '/thread/real-thread', provider: '/resources/providers/real-provider' };
  const rows = captureManifest(held);
  const routes = rows.filter((row) => row.kind === 'route');
  expect(routes).toHaveLength(SIGNED_IN_ROUTES.length + SIGNED_OUT_ROUTES.length);
  expect(new Set(rows.map((row) => row.id)).size).toBe(rows.length);
  expect(routes.filter((row) => row.pattern === '/project/$projectId/settings').every((row) => row.url.startsWith('/project/real-project/settings'))).toBe(true);
  expect(rows.find((row) => row.id === 'container-environment')?.url).toBe('/thread/real-thread');
  expect(rows.some((row) => row.url === '/thread/real-thread?catchup=true' && row.dialog === '*')).toBe(true);
  expect(rows.some((row) => row.url === '/thread/real-thread?turn=1')).toBe(true);
  expect(rows.some((row) => row.url === '/org/review-team/settings/members')).toBe(true);
  expect(rows.some((row) => row.url === '/org/acme.research')).toBe(true);
  expect(THREAD_CONTAINERS).toEqual(['conversation', 'team', 'code', 'environment', 'agent']);
  expect(THREAD_VIEWS).toContain('terminal');
  expect(rows.filter((row) => row.kind === 'container').map((row) => row.container)).toEqual([...THREAD_CONTAINERS]);
  expect(rows.filter((row) => row.kind === 'view').map((row) => row.view)).toEqual([...THREAD_VIEWS]);
  expect(ACTORS).toEqual(['ada', 'ada-admin', 'vera', 'ben']);
  expect(VIEWPORTS).toEqual([{ width: 1440, height: 900 }, { width: 400, height: 860 }]);
  expect(THEMES).toEqual(['light', 'dark']);
  expect(runTimeout({ ...held, locales: ['en', 'zh-CN'] })).toBeGreaterThan(60_000);
  expect(runTimeout({ ...held, locales: ['en', 'zh-CN'] })).toBeLessThanOrEqual(2_147_483_647);
});

test('a specific URL wins, mapped child sections keep their query, and segment mapping does not corrupt a neighbouring name', () => {
  const held = config();
  held.targets = { project: '/project/real-project' };
  held.routes = { [`/project/${ACME.web}?view=board`]: '/project/other-project?view=board' };
  held.replacements = { acme: 'review-team', [ACME.mainThread]: 'real-thread' };
  const rows = captureManifest(held);
  expect(rows.some((row) => row.url === '/project/other-project?view=board')).toBe(true);
  expect(mapRoute('/org/acme.research?scope=node:acme&items=one,two', held)).toBe('/org/acme.research?scope=node%3Areview-team&items=one%2Ctwo');
  expect(mapRoute('/u/constructor', held)).toBe('/u/constructor');
  expect(mapRoute('/search?tag=one&tag=two', held)).toBe('/search?tag=one&tag=two');
  expect(mapRoute(`/thread/${ACME.mainThread}`, held)).toBe('/thread/real-thread');
});

test('private input validation refuses incomplete identities and destinations outside the world', () => {
  expect(() => parseConfig(input, { ...environment, CAPTURE_TEST_BEN: undefined })).toThrow('people.ben.passwordEnv');
  for (const baseURL of ['file:///tmp/review', 'https://name:secret@example.invalid', 'https://example.invalid?ticket=secret', 'https://example.invalid/subpath']) {
    expect(() => parseConfig({ ...input, baseURL }, environment)).toThrow();
  }
  expect(() => parseConfig({ ...input, routes: { '/help': '//other.invalid/help' } }, environment)).toThrow();
  expect(() => parseConfig({ ...input, targets: { thread: '/thread/real?item=one' } }, environment)).toThrow();
  expect(() => captureManifest({ ...config(), targets: { misspelled: '/help' } })).toThrow('unknown capture target');
  expect(() => parseConfig({ ...input, locales: [] }, environment)).toThrow();
  expect(() => parseConfig({ ...input, dialogs: [{ id: 'empty', label: 'Empty', url: '/help', clicks: [], dialog: '*' }] }, environment)).toThrow();
  const override = parseConfig(input, { ...environment, ORCH_CAPTURE_URL: 'https://review.invalid' });
  expect(override.baseURL).toBe('https://review.invalid');
});

test('every built-in opener names an actual control in both shipped catalogues and custom opening lists stay explicit', () => {
  const held = config();
  held.dialogs = [{ id: 'restart-review', label: 'Restart review', url: '/resources/environments/retained-environment',
    clicks: [{ role: 'button', name: '@environmentRestart.action' }], dialog: '*' }];
  held.states = [{ id: 'selected-item', label: 'Selected tool call', url: '/thread/real-thread?item=real-item' }];
  const rows = captureManifest(held);
  for (const locale of ['en', 'zh-CN'] as const) {
    for (const row of rows) {
      for (const click of row.clicks ?? []) expect(caption(click.name, locale).trim(), `${row.id} ${locale}`).not.toBe('');
      if (row.dialog !== undefined && row.dialog !== '*') expect(caption(row.dialog, locale).trim(), row.id).not.toBe('');
    }
    for (const id of THREAD_CONTAINERS) expect(caption(`@workbench.containers.${id}.title`, locale)).not.toContain('@');
    for (const name of ['@workbench.containers.label', '@workbench.group.add', '@workbench.openView.choose', '@views.terminal.title']) expect(caption(name, locale)).not.toContain('@');
  }
  expect(rows.find((row) => row.id === 'extra-restart-review')?.clicks).toHaveLength(1);
  expect(rows.find((row) => row.id === 'extra-restart-review')?.dialog).toBe('*');
  expect(rows.find((row) => row.id === 'extra-url-selected-item')).toMatchObject({ kind: 'query', url: '/thread/real-thread?item=real-item' });
  expect(rows.find((row) => row.url === '/setup')?.note).toContain('never resets');
  expect(Object.keys(TARGET_SEEDS)).toContain('inspectedThread');
});

test('each new context retains the rotated session cookie but cannot reopen the previous capture\'s phone sheet or workbench', () => {
  const state = { cookies: [{ name: 'session', value: 'rotated-test-cookie', domain: 'review.invalid', path: '/', expires: -1, httpOnly: true, secure: true, sameSite: 'Lax' as const }],
    origins: [{ origin: 'https://review.invalid', localStorage: [
      { name: 'orch.person.workbench.thread', value: 'old-layout' }, { name: 'orch.person.choices.global.mobileContainer', value: '"environment"' },
      { name: 'orch.person.choices.resource-providers.tab', value: '"settings"' }, { name: 'orch.appearance', value: 'remembered-appearance' },
    ] }] };
  const fresh = freshStorage(state);
  expect(fresh.cookies[0]?.value).toBe('rotated-test-cookie');
  expect(fresh.origins[0]?.localStorage).toEqual([{ name: 'orch.appearance', value: 'remembered-appearance' }]);
  expect(state.origins[0]?.localStorage).toHaveLength(4);
});

test('API receipts retain failures, opening requests and their original phase while redacting every query value and ticket', () => {
  class PageEvents extends EventEmitter { isClosed(): boolean { return false; } }
  const events = new PageEvents();
  // This narrow event double deliberately implements only the Page event boundary used by the meter.
  const meter = new PageRequests(events as unknown as Page, 'https://review.invalid');
  const request = (url: string, method = 'GET', resourceType = 'fetch') => ({ url: () => url, method: () => method, resourceType: () => resourceType }) as Request;
  const first = request('https://review.invalid/api/v1/threads/00000000-0000-4000-8003-000000000004?cursor=one-use-secret');
  events.emit('request', first);
  meter.begin('opening');
  const opening = request('https://review.invalid/api/v1/providers/setup', 'POST');
  events.emit('request', opening);
  meter.begin('idle');
  events.emit('response', { request: () => first, status: () => 403 });
  events.emit('requestfinished', first);
  events.emit('requestfailed', opening);
  const idle = request('https://review.invalid/api/v1/me'); events.emit('request', idle); events.emit('requestfinished', idle);
  const asset = request('https://review.invalid/logo.svg', 'GET', 'image'); events.emit('request', asset); events.emit('requestfinished', asset);
  const foreign = request('https://other.invalid/api/v1/anything'); events.emit('request', foreign); events.emit('requestfinished', foreign);
  events.emit('websocket', { url: () => 'wss://review.invalid/api/v1/ws?ticket=private' });
  const receipt = meter.receipt(IDLE_MS); meter.stop();
  expect(receipt.first_load.count).toBe(1);
  expect(receipt.first_load.requests[0]).toMatchObject({ status: 403, phase: 'first_load', finished: true });
  expect(receipt.opening.requests[0]).toMatchObject({ method: 'POST', failed: true, phase: 'opening' });
  expect(receipt.idle.count).toBe(1);
  expect(receipt.live_connections).toHaveLength(1);
  expect(receipt.budgets).toMatchObject({ first_load_ok: true, idle_ok: false, idle_complete: true });
  expect(JSON.stringify(receipt)).not.toMatch(/one-use-secret|ticket=private/);
  expect(safeEndpoint('https://review.invalid/reset/secret-reset-ticket?proof=password')).toBe('/reset/:redacted?proof=redacted');
  expect(events.listenerCount('request')).toBe(0);
  const nine: RequestRow[] = Array.from({ length: 9 }, (): RequestRow => ({ phase: 'first_load', method: 'GET', endpoint: '/api/v1/me', at_ms: 0, status: 200, failed: false, finished: true }));
  expect(requestReceipt(nine, [], IDLE_MS - 1).budgets).toMatchObject({ first_load_ok: false, idle_complete: false });
});

test('readiness waits for the late body, correct theme and visible skeleton removal', async ({ page }) => {
  await page.setContent('<html lang="en" data-theme="dark"><body><div id="root"><nav>Navigation only</nav><main></main></div></body></html>');
  let settled = false;
  const ready = contentReady(page, 'en', 'light').then(() => { settled = true; });
  await page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
  expect(settled).toBe(false);
  await page.evaluate(() => {
    document.documentElement.dataset.theme = 'light';
    const main = document.querySelector('main'); if (main !== null) main.innerHTML = '<h1>Thread</h1><div aria-busy="true" style="height:20px">Loading</div>';
  });
  await page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
  expect(settled).toBe(false);
  await page.locator('[aria-busy]').evaluate((element) => element.remove());
  await ready;
  expect(settled).toBe(true);
});

test('a response which never ends cannot hold the capture walk forever', async () => {
  await expect(bounded(new Promise<never>(() => { /* A peer that sends headers, then no body. */ }), 5)).rejects.toThrow('exceeded its deadline');
  await expect(bounded(Promise.resolve('ready'), 100)).resolves.toBe('ready');
});

for (const theme of THEMES) test(`contact sheets retain all labels/failures, four columns and sixteen originals per PNG in ${theme}`, async ({ browser, page }, info) => {
  const directory = info.outputPath(`review-${theme}`);
  await mkdir(join(directory, 'screenshots'), { recursive: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.setContent('<main><h1>Retained original</h1><p>The screenshot must load from the sheet file.</p></main>');
  await page.screenshot({ path: join(directory, 'screenshots/original.png') });
  const rows: ScreenReceipt[] = Array.from({ length: 17 }, (_, index) => ({ ...screen(index + 1, theme), screenshot: 'screenshots/original.png' }));
  rows[1] = { ...screen(2, theme), label: '<script>not code</script> & a long route label '.repeat(6), issues: ['open_state_failed'], screenshot: null, ready: false };
  const html = contactHTML('Candidate <review>', rows, theme);
  expect(html).not.toContain('<script>not code</script>');
  expect(html).toContain('&lt;script&gt;not code&lt;/script&gt;');
  const run: CaptureRun = { format: 1, candidate: 'candidate', world: 'https://review.invalid', started: '2026-10-16T00:00:00Z', finished: null, expected_screens: 17,
    minimum_idle_minutes: 8.5, policy: { first_load_max: 8, idle_ms: 30000, idle_max: 0, network_quiet_ms: 500, workers: 1 }, session_issues: [], artifact_issues: [], screens: rows, sheets: [] };
  // Complete sixteen-screen pages become reviewable before the final partial page is rendered.
  await writeSheets(browser, directory, run, false);
  expect(run.sheets[0]?.pngs).toHaveLength(1);
  await writeSheets(browser, directory, run);
  expect(run.artifact_issues).toEqual([]);
  expect(run.sheets[0]?.pngs).toHaveLength(2);
  await page.goto(pathToFileURL(join(directory, 'sheets/1440x900-' + theme + '-en.html')).href);
  await expect(page.locator('#sheet-1 figure')).toHaveCount(16);
  await expect(page.locator('#sheet-2 figure')).toHaveCount(1);
  const columns = await page.locator('#sheet-1 main').evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
  expect(columns).toBe(4);
  await expect(page.locator('#sheet-1 figure.incomplete')).toHaveCount(1);
  await expect(page.locator('figcaption').nth(1)).toContainText('INCOMPLETE');
  const label = page.locator('figcaption').nth(1);
  expect(await label.evaluate((element) => element.scrollHeight <= element.clientHeight)).toBe(true);
  const png = await readFile(join(directory, `sheets/1440x900-${theme}-en-001.png`));
  expect(png.subarray(1, 4).toString()).toBe('PNG');
  expect(png.readUInt32BE(16)).toBe(1696);
  expect(png.readUInt32BE(20)).toBeGreaterThan(900);
  expect(await readFile(join(directory, 'requests.json'), 'utf8')).not.toMatch(/test-only-|rotated-test-cookie/);
  expect((await readFile(join(directory, 'index.html'), 'utf8'))).toContain(`sheets/1440x900-${theme}-en-002.png`);
  await writeFile(info.outputPath('contact-html.txt'), html);
});
