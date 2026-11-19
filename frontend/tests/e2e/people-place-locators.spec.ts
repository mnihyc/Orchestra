/** Native person/history and holder/source-place workflows through the production UI and existing authorized fixture API. */
import { randomUUID } from 'node:crypto';
import { expect, type Locator, type Page, type Request, type Response, type TestInfo, test } from '@playwright/test';
import type { HistoryLinePage, HolderSearchReply, Node as ApiNode, UserRef } from '../../src/generated/api-types.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { steer } from './give-support.ts';
import { ACME, deny, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

// Existing api/access.ts PEOPLE_PAGE. One older actor has one more row than this page;
// this many newer rows by someone else hide that actor from the unfiltered first page.
const HISTORY_PAGE = 8;
// Two existing 250 ms debounce intervals observe gated/duplicate reads, not a latency budget.
const OBSERVE_MS = 500;
const PRIVATE_DIAGNOSTIC = 'Private locator diagnostic; never interface copy';
// This former second field must stay absent; its label no longer belongs in the shipping catalogue.
const removedAddressLabel = { en: 'Another place by its address', 'zh-CN': '按地址查找其他位置' };
const modes = [{ locale: 'en', theme: 'light', width: 1440 }, { locale: 'zh-CN', theme: 'dark', width: 400 }] as const;
type Mode = (typeof modes)[number];
interface Call { method: string; path: string; query: Record<string, string> }
interface NativeReply { requestId: string | null; url: string; status: number; complete: boolean; readable: boolean; body: unknown }
type ObservedWindow = typeof window & { __peoplePlaceReplies: NativeReply[] };

async function observeJson(page: Page) {
  await page.addInitScript(() => {
    const observed = window as ObservedWindow; observed.__peoplePlaceReplies = [];
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const sent = send(input, init);
      const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
      void sent.then((response) => {
        const url = new URL(response.url);
        if (method !== 'GET' || !(url.pathname === '/api/v1/users/search' || url.pathname === '/api/v1/holders/search'
          || /^\/api\/v1\/nodes\/[^/]+(?:\/history)?$/u.test(url.pathname))) return;
        const receipt: NativeReply = { requestId: response.headers.get('x-request-id'), url: response.url, status: response.status, complete: false, readable: false, body: null };
        observed.__peoplePlaceReplies.push(receipt);
        void response.clone().json().then((body: unknown) => { receipt.body = body; receipt.readable = true; }, () => undefined)
          .finally(() => { receipt.complete = true; });
      }, () => undefined);
      // Preserve the original promise and header delivery; observation cannot hold application progress.
      return sent;
    };
  });
}

function observe(page: Page) {
  const calls: Call[] = [];
  const receive = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/')) calls.push({ method: request.method(), path: url.pathname, query: Object.fromEntries(url.searchParams) });
  };
  page.on('request', receive);
  return { calls, stop: () => page.off('request', receive) };
}
const historyPath = `/api/v1/nodes/${ACME.mainThread}/history`;
const histories = (calls: Call[]) => calls.filter((call) => call.path === historyPath);
const people = (calls: Call[], q?: string) => calls.filter((call) => call.path === '/api/v1/users/search' && (q === undefined || call.query.q === q));
const sources = (calls: Call[], q?: string) => calls.filter((call) => call.path === '/api/v1/holders/search' && call.query.list_places === 'true' && (q === undefined || call.query.q === q));
const holders = (calls: Call[]) => calls.filter((call) => call.path === '/api/v1/holders/search' && call.query.list_places === 'false');
const permissionWrites = (calls: Call[]) => calls.filter((call) => call.method !== 'GET' && /\/(?:permissions|roles|ownership|shares|groups)(?:\/|$)/u.test(call.path));

const responseBodies = new WeakMap<Response, unknown>();
async function responseFor(page: Page, path: string, matches: (query: URLSearchParams) => boolean = () => true) {
  const response = await page.waitForResponse((response) => {
    const url = new URL(response.url());
    return response.request().method() === 'GET' && url.pathname === path && matches(url.searchParams);
  });
  // Native request IDs correlate this exact response even when URL/status
  // repeat. Missing/duplicate/unreadable receipts fail; no older-body fallback.
  const requestId = response.headers()['x-request-id'];
  if (requestId === undefined || requestId === '') throw new Error('The native JSON response has no request ID');
  const key = { requestId, url: response.url(), status: response.status() };
  await expect.poll(() => page.evaluate((wanted) => {
    const rows = (window as ObservedWindow).__peoplePlaceReplies.filter((reply) => reply.requestId === wanted.requestId && reply.url === wanted.url && reply.status === wanted.status);
    return { count: rows.length, complete: rows[0]?.complete === true };
  }, key)).toEqual({ count: 1, complete: true });
  const receipt = await page.evaluate((wanted) => (window as ObservedWindow).__peoplePlaceReplies.find((reply) => reply.requestId === wanted.requestId && reply.url === wanted.url && reply.status === wanted.status), key);
  if (!receipt?.readable) throw new Error('The exact native JSON response clone was unreadable');
  responseBodies.set(response, receipt.body);
  return response;
}

function json<T>(response: Response): Promise<T> {
  expect(response.status()).toBe(200);
  expect(responseBodies.has(response)).toBe(true);
  return Promise.resolve(responseBodies.get(response) as T);
}

/** Hold one actual successful native response; its body and permissions are never replaced. */
async function holdNext(page: Page, path: string, matches: (query: URLSearchParams) => boolean = () => true) {
  let release: () => void = () => undefined, entered: (value: unknown) => void = () => undefined, finished: () => void = () => undefined;
  let rejected: (error: unknown) => void = () => undefined;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const started = new Promise<unknown>((resolve, reject) => { entered = resolve; rejected = reject; });
  const done = new Promise<void>((resolve) => { finished = resolve; });
  let used = false;
  await page.route(`**${path}*`, async (route) => {
    const request = route.request(), url = new URL(request.url());
    if (used || request.method() !== 'GET' || url.pathname !== path || !matches(url.searchParams)) { await route.fallback(); return; }
    used = true;
    try {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      entered(await response.json());
      await gate;
      try { await route.fulfill({ response }); }
      catch (error) { if (request.failure() === null) throw error; } // Leaving a query may abort its original browser request.
    } catch (error) { rejected(error); throw error; }
    finally { finished(); }
  });
  return { started, release, done };
}

/** A one-attempt transport failure; retry goes to the ordinary endpoint with its real result. */
async function failNext(page: Page, path: string, matches: (query: URLSearchParams) => boolean) {
  let used = false;
  await page.route(`**${path}*`, async (route) => {
    const url = new URL(route.request().url());
    if (used || route.request().method() !== 'GET' || url.pathname !== path || !matches(url.searchParams)) { await route.fallback(); return; }
    used = true;
    const requestId = randomUUID();
    await route.fulfill({ status: 500, headers: { 'X-Request-Id': requestId }, contentType: 'application/problem+json', json: {
      type: 'orch:error:internal', code: 'internal', status: 500, title: PRIVATE_DIAGNOSTIC, detail: PRIVATE_DIAGNOSTIC, request_id: requestId,
    } });
  });
}

async function prepare(page: Page, mode: Mode) {
  await observeJson(page);
  await page.setViewportSize({ width: mode.width, height: 900 });
  await page.emulateMedia({ colorScheme: mode.theme }); await signIn(page);
  if (mode.locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', mode.locale);
  await expect(page.locator('html')).toHaveAttribute('data-theme', mode.theme);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-thread-header]')).toBeVisible();
}

async function capture(page: Page, info: TestInfo, mode: Mode, name: string, target: Locator) {
  await target.scrollIntoViewIfNeeded(); await page.mouse.move(0, 0);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => {
      const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end);
    }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const stem = `${name}-${mode.locale}-${mode.theme}-${mode.width}`, path = info.outputPath(`${stem}.png`);
  await page.screenshot({ path, fullPage: false, caret: 'hide' });
  await info.attach(stem, { path, contentType: 'image/png' });
}

function places(reply: HolderSearchReply) {
  const safe = new Map<string, { id: string; path: string; locator: string }>();
  for (const item of reply.items) if (item.holder.kind === 'role' && item.holder.at !== null && !item.holder.at.redacted && item.holder.at.name !== null) {
    safe.set(item.holder.at.id, { id: item.holder.at.id, path: item.place, locator: item.locator });
  }
  return [...safe.values()].sort((one, other) => one.path.localeCompare(other.path));
}

for (const mode of modes) {
  test(`History finds an older actor before paging and preserves privacy (${mode.locale}/${mode.theme})`, async ({ page }, info) => {
    await scenario();
    for (const actor of [...Array.from({ length: HISTORY_PAGE + 1 }, () => 'grace'), ...Array.from({ length: HISTORY_PAGE }, () => 'ada')]) {
      await steer('/touch', { as: actor, path: 'nodes', id: ACME.mainThread, change: { rows: [{ permission: 'thread.view', inherit: true }] } });
    }
    await prepare(page, mode);
    const words = mode.locale === 'en' ? english : chinese, observed = observe(page);
    let releaseHistory: () => void = () => undefined;
    try {
      const initial = responseFor(page, historyPath, (q) => !q.has('actor') && !q.has('cursor'));
      await showWorkbenchView(page, 'History');
      const first = await json<HistoryLinePage>(await initial);
      expect(first.items).toHaveLength(HISTORY_PAGE); expect(first.next_cursor).not.toBeNull();
      const history = page.getByTestId('thread-history');
      const viewId = await history.evaluate((element) => element.closest('[data-view-instance]')?.getAttribute('data-view-instance'));
      if (viewId === null || viewId === undefined) throw new Error('History has no owning workbench view');
      const view = page.locator(`[data-view-instance="${viewId}"]`);
      const input = history.getByRole('combobox', { name: words.threadViews.history.person, exact: true });
      const lines = history.getByRole('list', { name: words.access.history.label, exact: true });
      await expect(lines.getByRole('listitem')).toHaveCount(HISTORY_PAGE);
      expect(histories(observed.calls)[0]?.query).toEqual({ limit: String(HISTORY_PAGE), depth: 'summary' });
      await input.focus(); await page.waitForTimeout(OBSERVE_MS);
      await input.fill('😀'); await page.waitForTimeout(OBSERVE_MS); await input.press('Enter');
      expect(people(observed.calls)).toHaveLength(0); expect(histories(observed.calls)).toHaveLength(1);
      const lookup = responseFor(page, '/api/v1/users/search', (q) => q.get('q') === 'grace');
      await input.fill('grace');
      const found = await json<UserRef[]>(await lookup), grace = found.find((person) => person.username === 'grace');
      if (grace === undefined) throw new Error('The existing authorized people lookup did not offer Grace');
      expect(first.items.every((line) => line.actor !== null && !('agent' in line.actor) && line.actor.id !== grace.id)).toBe(true);
      expect(people(observed.calls, 'grace').map((call) => call.query)).toEqual([{ q: 'grace' }]);
      await expect(page.getByRole('option', { name: grace.display_name, exact: true })).toBeVisible();
      expect(histories(observed.calls)).toHaveLength(1);
      await capture(page, info, mode, 'history-native-person', history);

      const held = await holdNext(page, historyPath, (q) => q.get('actor') === grace.id && !q.has('cursor'));
      releaseHistory = held.release;
      const selected = responseFor(page, historyPath, (q) => q.get('actor') === grace.id && !q.has('cursor'));
      await input.press('Enter');
      const heldPage = await held.started as HistoryLinePage;
      expect(heldPage.items).toHaveLength(HISTORY_PAGE);
      await expect(input).toHaveValue(grace.display_name);
      await expect(lines).toHaveCount(0);
      held.release();
      const filtered = await json<HistoryLinePage>(await selected); await held.done;
      expect(filtered).toEqual(heldPage);
      expect(filtered.items.every((line) => line.actor !== null && !('agent' in line.actor) && line.actor.id === grace.id)).toBe(true);
      await expect(lines.getByRole('listitem')).toHaveCount(HISTORY_PAGE);
      expect(histories(observed.calls).at(-1)?.query).toEqual({ limit: String(HISTORY_PAGE), depth: 'summary', actor: grace.id });

      // Another current lookup may change its hits, but never the committed history actor.
      const beforeLookup = histories(observed.calls).length;
      const other = responseFor(page, '/api/v1/users/search', (q) => q.get('q') === 'dora');
      await input.fill('dora'); const others = await json<UserRef[]>(await other);
      const dora = others.find((person) => person.username === 'dora');
      if (dora === undefined) throw new Error('The existing lookup did not offer Dora');
      await expect(page.getByRole('option', { name: dora.display_name, exact: true })).toBeVisible();
      await input.press('Escape'); await expect(input).toHaveValue(grace.display_name);
      expect(histories(observed.calls)).toHaveLength(beforeLookup);
      await failNext(page, '/api/v1/users/search', (q) => q.get('q') === 'margaret');
      const failed = responseFor(page, '/api/v1/users/search', (q) => q.get('q') === 'margaret');
      await input.fill('margaret'); expect((await failed).status()).toBe(500);
      await expect(history.getByRole('alert')).toBeVisible();
      await expect(page.getByRole('option', { name: dora.display_name, exact: true })).toHaveCount(0);
      await input.press('Escape'); await expect(input).toHaveValue(grace.display_name);
      await expect(history).not.toContainText(PRIVATE_DIAGNOSTIC);
      await capture(page, info, mode, 'history-lookup-recovery', history);
      const recovered = responseFor(page, '/api/v1/users/search', (q) => q.get('q') === 'margaret');
      await history.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      const recoveredPeople = await json<UserRef[]>(await recovered);
      const margaret = recoveredPeople.find((person) => person.username === 'margaret');
      if (margaret === undefined) throw new Error('The native retry did not offer Margaret');
      await input.focus(); await expect(page.getByRole('option', { name: margaret.display_name, exact: true })).toBeVisible();
      await input.press('Escape'); await page.waitForTimeout(OBSERVE_MS);
      await expect(input).toHaveValue(grace.display_name);
      expect(people(observed.calls, 'margaret').map((call) => call.query)).toEqual([{ q: 'margaret' }, { q: 'margaret' }]);
      expect(histories(observed.calls)).toHaveLength(beforeLookup);

      const next = responseFor(page, historyPath, (q) => q.get('cursor') === filtered.next_cursor && q.get('actor') === grace.id);
      await history.getByRole('button', { name: words.ui.pagination.more, exact: true }).click();
      const second = await json<HistoryLinePage>(await next);
      expect(second.items).toHaveLength(1); expect(second.next_cursor).toBeNull();
      expect(second.items[0]?.actor).toEqual(grace);
      expect(new Set([...filtered.items, ...second.items].map((line) => line.id)).size).toBe(HISTORY_PAGE + 1);
      await expect(lines.getByRole('listitem')).toHaveCount(HISTORY_PAGE + 1);
      expect(histories(observed.calls).at(-1)?.query).toEqual({ limit: String(HISTORY_PAGE), depth: 'summary', actor: grace.id, cursor: filtered.next_cursor });
      await capture(page, info, mode, 'history-filtered-pages', history);

      // Clearing/reselecting may reuse fresh pages; neither identity is reconstructed from those pages.
      await history.getByRole('button', { name: words.trace.clearPerson, exact: true }).click();
      await expect(input).toHaveValue(words.trace.anyPerson);
      await expect(lines.getByRole('listitem')).toHaveCount(HISTORY_PAGE); await expect(lines).not.toContainText(grace.display_name);
      await input.fill('grace'); await expect(page.getByRole('option', { name: grace.display_name, exact: true })).toBeVisible();
      await input.press('Enter'); await expect(input).toHaveValue(grace.display_name);
      await expect(lines.getByRole('listitem')).toHaveCount(HISTORY_PAGE + 1);

      // A new depth keeps the actor. Denial on its second page must suppress its cached first page and selected name.
      const every = responseFor(page, historyPath, (q) => q.get('depth') === 'every' && q.get('actor') === grace.id && !q.has('cursor'));
      await history.getByRole('radiogroup', { name: words.places.activity.depth, exact: true }).getByRole('radio').nth(1).click();
      const everyFirst = await json<HistoryLinePage>(await every); expect(everyFirst.next_cursor).not.toBeNull();
      await expect(input).toHaveValue(grace.display_name);
      await deny('ada', [`thread.view@${ACME.mainThread}`]);
      const refused = responseFor(page, historyPath, (q) => q.get('depth') === 'every' && q.get('actor') === grace.id && q.get('cursor') === everyFirst.next_cursor);
      await history.getByRole('button', { name: words.ui.pagination.more, exact: true }).click();
      expect((await refused).status()).toBe(403);
      await expect(view.getByRole('alert')).toBeVisible();
      await expect(view.getByRole('list', { name: words.access.history.label, exact: true })).toHaveCount(0);
      await expect(view.getByRole('combobox', { name: words.threadViews.history.person, exact: true })).toHaveCount(0);
      await expect(view).not.toContainText(grace.display_name);
      await expect(view).not.toContainText(PRIVATE_DIAGNOSTIC);
      await capture(page, info, mode, 'history-native-refusal', view);
      expect(permissionWrites(observed.calls)).toEqual([]);
    } finally {
      releaseHistory(); observed.stop();
      await info.attach('history-native-calls', { body: JSON.stringify(observed.calls, null, 2), contentType: 'application/json' });
    }
  });

  test(`Holder source is one native locator with Current and exact retry recovery (${mode.locale}/${mode.theme})`, async ({ page }, info) => {
    await scenario(); await prepare(page, mode);
    const words = mode.locale === 'en' ? english : chinese, observed = observe(page);
    const releases: (() => void)[] = [];
    try {
      await page.locator('[data-thread-header]').getByRole('button', { name: words.actions.share.label, exact: true }).click();
      // Share uses the addressed view's current authorized name. Its parent
      // qualifier can arrive after the native conversation becomes usable.
      const dialog = page.getByRole('dialog', { name: words.access.title.replace('{{name}}', 'Main thread'), exact: true })
        .or(page.getByRole('dialog', { name: words.access.title.replace('{{name}}', 'Main thread · Fix login'), exact: true }));
      await expect(dialog).toHaveCount(1);
      await expect(dialog).toBeVisible();
      const source = dialog.getByRole('combobox', { name: words.access.candidates.place, exact: true });
      const holder = dialog.getByRole('combobox', { name: words.access.add.label, exact: true });
      const current = words.access.candidates.thisPlace.replace('{{place}}', 'Main thread');
      const sourceList = page.getByRole('listbox', { name: words.access.candidates.place, exact: true });
      const holderList = page.getByRole('listbox', { name: words.access.add.label, exact: true });
      const options = dialog.getByRole('button', { name: words.access.roster.options, exact: true });
      await expect(source).toHaveCount(0); expect(sources(observed.calls)).toHaveLength(0);
      const initialHolders = responseFor(page, '/api/v1/holders/search', (q) => q.get('list_places') === 'false' && q.get('place_id') === ACME.fixLogin && q.get('org_id') === ACME.org);
      await options.click(); await json<HolderSearchReply>(await initialHolders);
      await expect(source).toHaveCount(1); expect(await source.evaluate((element) => element.tagName)).toBe('INPUT');
      await expect(dialog.getByRole('combobox', { name: removedAddressLabel[mode.locale], exact: true })).toHaveCount(0);
      await expect(source).toHaveValue(current);
      await page.waitForTimeout(OBSERVE_MS); expect(sources(observed.calls)).toHaveLength(0);
      const browse = responseFor(page, '/api/v1/holders/search', (q) => q.get('list_places') === 'true' && q.get('q') === '');
      await source.click(); const offered = places(await json<HolderSearchReply>(await browse));
      expect(offered.length).toBeGreaterThan(10);
      expect(sources(observed.calls).at(-1)?.query).toEqual({ q: '', place_id: ACME.org, org_id: ACME.org, include_work: 'false', list_places: 'true' });
      await expect(sourceList.getByRole('option')).toHaveCount(offered.length + 1);
      const lateAt = offered.findIndex((place) => place.id === ACME.cardFees), late = offered[lateAt];
      expect(lateAt).toBeGreaterThanOrEqual(10);
      if (late === undefined) throw new Error('The real authorized browse did not contain the late Card fees place');
      await capture(page, info, mode, 'holder-native-browse', dialog);
      const fromLate = responseFor(page, '/api/v1/holders/search', (q) => q.get('list_places') === 'false' && q.get('place_id') === late.id);
      await sourceList.getByRole('option', { name: late.path, exact: true }).click();
      await json<HolderSearchReply>(await fromLate);
      await expect(source).toHaveValue(late.path); await expect(sourceList).toHaveCount(0);
      expect(holders(observed.calls).at(-1)?.query).toEqual({ q: '', place_id: late.id, org_id: ACME.org, include_work: 'false', list_places: 'false' });

      // A selected source's own root is a read, but Current remains a local keyboard action.
      const rootPath = `/api/v1/nodes/${late.id}`, pendingRoot = await holdNext(page, rootPath);
      releases.push(pendingRoot.release);
      const beforeRootBrowse = sources(observed.calls).length;
      await source.click(); const nativeRoot = await pendingRoot.started as ApiNode;
      expect(nativeRoot.id).toBe(late.id); expect(nativeRoot.org_id).toBe(ACME.org);
      await expect(sourceList).toHaveAttribute('aria-busy', 'true');
      const reset = dialog.getByRole('button', { name: current, exact: true });
      await expect(reset).toBeEnabled(); await capture(page, info, mode, 'holder-pending-root-current', dialog);
      const beforeReset = holders(observed.calls).length;
      await reset.focus(); await reset.press('Enter');
      await expect(source).toHaveValue(current); await expect(sourceList).toHaveCount(0);
      expect(sources(observed.calls)).toHaveLength(beforeRootBrowse);
      pendingRoot.release(); await pendingRoot.done; await page.waitForTimeout(OBSERVE_MS);
      await expect(source).toHaveValue(current); expect(sources(observed.calls)).toHaveLength(beforeRootBrowse);
      expect(observed.calls.filter((call) => call.path === rootPath)).toHaveLength(1);
      // A still-fresh Current answer is reused; if its 15 s freshness expired, at most one exact refresh is legitimate.
      const resetReads = holders(observed.calls).slice(beforeReset);
      expect(resetReads.length).toBeLessThanOrEqual(1);
      for (const call of resetReads) expect(call.query).toEqual({ q: '', place_id: ACME.fixLogin, org_id: ACME.org, include_work: 'false', list_places: 'false' });
      // The existing add-holder field requires a nonempty query. A native role
      // word searches within the selected source, without naming another place.
      await holder.fill('viewer'); await expect(holderList).toContainText('Fix login');
      await expect(holderList).not.toContainText('Card fees'); await holder.press('Escape');
      await holder.fill(''); await page.waitForTimeout(OBSERVE_MS);

      await source.click(); await expect(sourceList.getByRole('option', { name: current, exact: true })).toBeEnabled();
      const shortReads = sources(observed.calls).length, unchangedHolders = holders(observed.calls).length;
      await source.fill('a'); await page.waitForTimeout(OBSERVE_MS); await source.press('Enter');
      expect(sources(observed.calls)).toHaveLength(shortReads); expect(holders(observed.calls)).toHaveLength(unchangedHolders);
      const missingText = 'no-such-source-people-place-regression';
      const noMatch = responseFor(page, '/api/v1/holders/search', (q) => q.get('q') === missingText && q.get('list_places') === 'true');
      await source.fill(missingText); expect(places(await json<HolderSearchReply>(await noMatch))).toEqual([]);
      await expect(sourceList.getByRole('option')).toHaveCount(0); await source.press('Enter');
      await source.press('Escape'); await expect(source).toHaveValue(current);
      expect(holders(observed.calls)).toHaveLength(unchangedHolders); expect(permissionWrites(observed.calls)).toEqual([]);

      // Search uses the original home for outsider classification, with no place_id filter.
      const beforeSearchHolders = holders(observed.calls).length;
      const personalSearch = responseFor(page, '/api/v1/holders/search', (q) => q.get('q') === 'ada' && q.get('list_places') === 'true');
      await source.fill('ada');
      const personal = places(await json<HolderSearchReply>(await personalSearch)).find((place) => place.locator === 'ada');
      if (personal === undefined) throw new Error('Ada’s existing authorized personal root was not found by its native address');
      expect(sources(observed.calls, 'ada').at(-1)?.query).toEqual({ q: 'ada', org_id: ACME.org, include_work: 'false', list_places: 'true' });
      await expect(sourceList.getByRole('option', { name: personal.path, exact: true })).toBeVisible();
      expect(holders(observed.calls)).toHaveLength(beforeSearchHolders); expect(permissionWrites(observed.calls)).toEqual([]);
      const personalHolders = responseFor(page, '/api/v1/holders/search', (q) => q.get('place_id') === personal.id && q.get('list_places') === 'false');
      await sourceList.getByRole('option', { name: personal.path, exact: true }).click();
      await json<HolderSearchReply>(await personalHolders); await expect(source).toHaveValue(personal.path);
      expect(holders(observed.calls).at(-1)?.query).toEqual({ q: '', place_id: personal.id, org_id: ACME.org, include_work: 'false', list_places: 'false' });
      const personalBrowse = responseFor(page, '/api/v1/holders/search', (q) => q.get('place_id') === personal.id && q.get('list_places') === 'true');
      await source.click(); await json<HolderSearchReply>(await personalBrowse);
      expect(sources(observed.calls).at(-1)?.query).toEqual({ q: '', place_id: personal.id, org_id: personal.id, include_work: 'false', list_places: 'true' });

      const retryText = 'acme.research.ml';
      await failNext(page, '/api/v1/holders/search', (q) => q.get('q') === retryText && q.get('list_places') === 'true');
      const failed = responseFor(page, '/api/v1/holders/search', (q) => q.get('q') === retryText && q.get('list_places') === 'true');
      await source.fill(retryText); expect((await failed).status()).toBe(500);
      await expect(dialog.getByRole('alert')).toBeVisible(); await expect(sourceList.getByRole('option')).toHaveCount(0);
      await expect(dialog).not.toContainText(PRIVATE_DIAGNOSTIC);
      await capture(page, info, mode, 'holder-search-recovery', dialog);
      const retried = responseFor(page, '/api/v1/holders/search', (q) => q.get('q') === retryText && q.get('list_places') === 'true');
      // This pointer click blurs the input. ErrorState must survive it and cause one read, without reopening automatically.
      await dialog.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      const recovered = places(await json<HolderSearchReply>(await retried)).find((place) => place.id === ACME.ml);
      if (recovered === undefined) throw new Error('The actual native retry did not return Research / ML');
      await expect(source).toHaveValue(personal.path); await expect(sourceList).toHaveCount(0);
      await source.click(); await expect(sourceList.getByRole('option', { name: recovered.path, exact: true })).toBeVisible();
      await page.waitForTimeout(OBSERVE_MS);
      expect(sources(observed.calls, retryText).map((call) => call.query)).toEqual([
        { q: retryText, org_id: ACME.org, include_work: 'false', list_places: 'true' },
        { q: retryText, org_id: ACME.org, include_work: 'false', list_places: 'true' },
      ]);
      await source.press('Escape'); await expect(source).toHaveValue(personal.path);

      // The same Current control remains a pointer action while a native address search is pending.
      const waitingText = 'acme.infra';
      const pendingSearch = await holdNext(page, '/api/v1/holders/search', (q) => q.get('q') === waitingText && q.get('list_places') === 'true');
      releases.push(pendingSearch.release);
      await source.fill(waitingText); const waitingReply = await pendingSearch.started as HolderSearchReply;
      expect(places(waitingReply).some((place) => place.id === ACME.infra)).toBe(true);
      await expect(sourceList).toHaveAttribute('aria-busy', 'true');
      await capture(page, info, mode, 'holder-pending-search-current', dialog);
      const beforePendingReset = sources(observed.calls).length;
      await dialog.getByRole('button', { name: current, exact: true }).click();
      await expect(source).toHaveValue(current); await expect(sourceList).toHaveCount(0);
      pendingSearch.release(); await pendingSearch.done; await page.waitForTimeout(OBSERVE_MS);
      await expect(source).toHaveValue(current); expect(sources(observed.calls)).toHaveLength(beforePendingReset);
      expect(sources(observed.calls, waitingText)).toHaveLength(1);

      // Only this previously unvisited source loses View. Its old names/holder rows must not survive the failed selected-root read.
      const findBaseline = responseFor(page, '/api/v1/holders/search', (q) => q.get('q') === 'baseline' && q.get('list_places') === 'true');
      await source.fill('baseline');
      const baseline = places(await json<HolderSearchReply>(await findBaseline)).find((place) => place.id === ACME.baseline);
      if (baseline === undefined) throw new Error('The real visible Baseline run source is missing');
      const baselineHolders = responseFor(page, '/api/v1/holders/search', (q) => q.get('place_id') === baseline.id && q.get('list_places') === 'false');
      await sourceList.getByRole('option', { name: baseline.path, exact: true }).click(); await json<HolderSearchReply>(await baselineHolders);
      await deny('ada', [`topic.view@${baseline.id}`]);
      const beforeRefusal = sources(observed.calls).length;
      const refused = responseFor(page, `/api/v1/nodes/${baseline.id}`);
      await source.click(); expect((await refused).status()).toBe(403);
      await expect(dialog.getByRole('alert')).toBeVisible();
      await expect(sourceList.getByRole('option', { name: words.access.place.hidden, exact: true })).toHaveAttribute('aria-disabled', 'true');
      await expect(dialog).not.toContainText(baseline.path);
      await source.press('Escape'); await expect(source).toHaveValue(words.access.place.hidden);
      await holder.fill('viewer'); await expect(holderList).toBeVisible();
      await expect(holderList.getByRole('option')).toHaveCount(0);
      await expect(holderList).not.toContainText('Baseline run');
      expect(sources(observed.calls)).toHaveLength(beforeRefusal);
      await capture(page, info, mode, 'holder-native-refusal', dialog);
      await holder.press('Escape'); await holder.fill(''); await page.waitForTimeout(OBSERVE_MS);
      await dialog.getByRole('button', { name: current, exact: true }).click();
      await expect(source).toHaveValue(current); await expect(dialog.getByRole('alert')).toHaveCount(0);
      await holder.fill('viewer'); await expect(holderList).toContainText('Fix login'); await expect(holderList).not.toContainText('Baseline run');
      await holder.press('Escape'); await holder.fill(''); await page.waitForTimeout(OBSERVE_MS);
      await options.click(); await expect(source).toHaveCount(0);
      const closedReads = sources(observed.calls).length;
      await page.waitForTimeout(OBSERVE_MS); expect(sources(observed.calls)).toHaveLength(closedReads);
      expect(permissionWrites(observed.calls)).toEqual([]);
      await expect(dialog.getByRole('list', { name: words.access.add.staged, exact: true })).toHaveCount(0);
    } finally {
      for (const release of releases) release();
      observed.stop(); await info.attach('holder-native-calls', { body: JSON.stringify(observed.calls, null, 2), contentType: 'application/json' });
    }
  });
}
