/** Production choice workflows over complete declarations/catalogues, using the existing API double and its ACLs. */
import { expect, type Locator, type Page, type Request, type TestInfo, test } from '@playwright/test';
import type { Approval, UsageAnalytics, UsageEventPage } from '../../src/generated/api-types.ts';
import { ACTIONS } from '../../src/features/trace/catalogue.ts';
import { OUTCOMES } from '../../src/features/usage/outcomes.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { harnessApproval } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, addApproval, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const MODES = [{ locale: 'en', theme: 'light' }, { locale: 'zh-CN', theme: 'dark' }] as const;
type Mode = (typeof MODES)[number];
// Two existing 250 ms search-debounce intervals expose an accidental delayed search/write.
// This is a bounded negative-I/O observation, not a product latency requirement.
const OBSERVE_MS = 500;
const DESKTOP = { width: 1440, height: 900 };
const NARROW = { width: 400, height: 900 };

interface Call { method: string; path: string; query: Record<string, string> }
function observeApi(page: Page) {
  const calls: Call[] = [];
  const receive = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/')) calls.push({ method: request.method(), path: url.pathname, query: Object.fromEntries(url.searchParams) });
  };
  page.on('request', receive);
  return { calls, stop: () => page.off('request', receive) };
}

async function prepare(page: Page, mode: Mode): Promise<void> {
  await page.setViewportSize(DESKTOP);
  await page.emulateMedia({ colorScheme: mode.theme });
  await signIn(page);
  if (mode.locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', mode.locale);
  await expect(page.locator('html')).toHaveAttribute('data-theme', mode.theme);
}

async function noTypingIo(page: Page, calls: readonly Call[], edit: () => Promise<void>): Promise<void> {
  // Settle ordinary page-opening/state persistence before the measured interaction.
  await page.waitForTimeout(OBSERVE_MS);
  const before = [...calls];
  const address = page.url();
  await edit();
  await page.waitForTimeout(OBSERVE_MS);
  expect(calls, 'local choice search must issue no API read or write').toEqual(before);
  expect(page.url(), 'typing must not apply a filter or submit its form').toBe(address);
}

async function capture(page: Page, info: TestInfo, name: string, width: number, list?: Locator): Promise<void> {
  expect(page.viewportSize()?.width).toBe(width);
  // One CSS pixel allows rounding; the actual popup and page must fit the viewport.
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 1);
  if (list !== undefined) {
    await expect(list).toBeVisible();
    const box = await list.boundingBox();
    if (box === null) throw new Error('The open choice list has no visible box');
    expect(box.x).toBeGreaterThanOrEqual(-1);
    expect(box.x + box.width).toBeLessThanOrEqual(width + 1);
  }
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path, fullPage: false, animations: 'disabled', caret: 'hide' });
  await info.attach(name, { path, contentType: 'image/png' });
}

async function recordCalls(info: TestInfo, calls: readonly Call[]): Promise<void> {
  await info.attach('native-choice-api-calls', { body: JSON.stringify(calls, null, 2), contentType: 'application/json' });
}

test.beforeEach(async () => { await scenario(); });

for (const mode of MODES) {
  test(`complete declaration keeps its current value and help until explicit Accept (${mode.locale}/${mode.theme})`, async ({ page }, info) => {
    const words = mode.locale === 'en' ? english : chinese;
    const title = mode.locale === 'en' ? 'Choose a processing queue' : '选择处理队列';
    const label = mode.locale === 'en' ? 'Processing queue' : '处理队列';
    const description = mode.locale === 'en' ? 'Use the queue declared by the requesting server. Accept sends the answer.' : '使用请求服务器声明的队列。点击接受后才会发送答案。';
    const values = Array.from({ length: 12 }, (_, index) => `queue-${String(index + 1).padStart(2, '0')}`);
    const id = `complete-choice-${mode.locale}`;
    // The normal harness fixture is complete by declaration. Ada's existing thread.manage decides it.
    await addApproval(harnessApproval(id, { threadId: ACME.mainThread, itemId: `${id}-item`, title, requester: ADA,
      request: { request_kind: 'mcp_elicitation', options: [], detail: { server: 'queue-service', message: title,
        schema: { type: 'object', required: ['queue'], properties: { queue: { type: 'string', title: label, description, enum: values, default: 'queue-11' } } } } } }));
    await prepare(page, mode);
    const decisionPath = `/api/v1/approvals/${id}/decision`;
    await page.addInitScript((path) => {
      const observed = window as typeof window & { __orchChoiceDecisions: unknown[] };
      observed.__orchChoiceDecisions = [];
      const send = window.fetch.bind(window);
      window.fetch = (input, init) => {
        if (input instanceof Request && input.method === 'POST' && new URL(input.url).pathname === path) {
          void input.clone().json().then((body: unknown) => { observed.__orchChoiceDecisions.push(body); }, () => { observed.__orchChoiceDecisions.push('Unreadable JSON'); });
        }
        return send(input, init);
      };
    }, decisionPath);
    const observed = observeApi(page);
    try {
      await page.goto('/approvals');
      const card = page.getByRole('region', { name: title, exact: true });
      const input = card.getByRole('combobox', { name: label, exact: true });
      const accept = card.getByRole('button', { name: words.approvals.accept, exact: true });
      await expect(input).toHaveValue('queue-11');
      await expect(input).toHaveAccessibleDescription(description);
      await expect(accept).toBeEnabled();
      await noTypingIo(page, observed.calls, async () => {
        await input.click();
        const list = page.getByRole('listbox', { name: label, exact: true });
        await expect(list.getByRole('option')).toHaveCount(12);
        await expect(list.getByRole('option', { name: 'queue-11', exact: true })).toHaveAttribute('aria-selected', 'true');
        await expect(list.getByRole('option', { name: 'queue-12', exact: true })).toHaveAccessibleDescription(description);
        await capture(page, info, `declaration-${mode.locale}-${mode.theme}-1440`, DESKTOP.width, list);
        // Enter on the already selected default closes the locator without submitting Accept.
        await input.press('Enter');
        await expect(input).toHaveValue('queue-11');
        await input.click();
        await input.pressSequentially('does-not-exist');
        await expect(page.getByRole('option')).toHaveCount(0);
        await input.press('Enter');
        await expect(accept).toBeEnabled();
        await input.press('Escape');
        await expect(input).toBeFocused();
        await expect(input).toHaveValue('queue-11');
        await input.press('Enter');
        await expect(input).toHaveAttribute('aria-expanded', 'false');
      });
      expect(observed.calls.filter((call) => call.path === decisionPath)).toEqual([]);

      // The same production form remains usable in a narrow pane, with its current value retained.
      await page.setViewportSize(NARROW);
      await expect(input).toHaveValue('queue-11');
      await noTypingIo(page, observed.calls, async () => {
        await input.click(); await input.pressSequentially('queue-12');
        const list = page.getByRole('listbox', { name: label, exact: true });
        await expect(list.getByRole('option')).toHaveCount(1);
        await expect(list.getByRole('option', { name: 'queue-12', exact: true })).toHaveAccessibleDescription(description);
        await capture(page, info, `declaration-${mode.locale}-${mode.theme}-400`, NARROW.width, list);
        await input.press('ArrowDown'); await input.press('Enter');
        await expect(input).toHaveValue('queue-12');
        await expect(input).toHaveAccessibleDescription(description);
      });
      expect(observed.calls.filter((call) => call.path === decisionPath)).toEqual([]);
      const answered = page.waitForResponse((response) => new URL(response.url()).pathname === decisionPath && response.request().method() === 'POST');
      await accept.click();
      const response = await answered;
      expect(response.status()).toBe(200);
      expect(await page.evaluate(() => (window as typeof window & { __orchChoiceDecisions: unknown[] }).__orchChoiceDecisions)).toEqual([{ action: 'accept', content: { queue: 'queue-12' } }]);
      const approved = await response.json() as Approval;
      expect(approved.state).toBe('approved');
      expect(approved.decision).toEqual({ action: 'accept', content: { queue: 'queue-12' } });
      await expect(accept).toHaveCount(0);
      expect(observed.calls.filter((call) => call.path === decisionPath)).toHaveLength(1);
    } finally { observed.stop(); await recordCalls(info, observed.calls); }
  });

  test(`Usage splits and outcomes search complete catalogues and send native filter arguments (${mode.locale}/${mode.theme})`, async ({ page, request }, info) => {
    expect((await request.post(`${DOUBLE}/__double/usage/performance`)).ok()).toBe(true);
    await prepare(page, mode);
    const words = mode.locale === 'en' ? english : chinese;
    const scope = `node:${ACME.org}`;
    const observed = observeApi(page);
    const usageResponse = (series: string) => page.waitForResponse((response) => {
      const url = new URL(response.url());
      return response.request().method() === 'GET' && url.pathname === '/api/v1/usage' && url.searchParams.get('scope') === scope && url.searchParams.get('series_by') === series;
    });
    try {
      for (const view of ['usage', 'performance'] as const) {
        const loaded = usageResponse('model');
        await page.goto(`/usage?scope=${encodeURIComponent(scope)}&view=${view}&range=7d&series=model`);
        const response = await loaded;
        expect(response.status()).toBe(200);
        const report = await response.json() as UsageAnalytics;
        // Thread is offered only at these native depths. The test does not grant itself reports.
        expect(['own', 'reports']).toContain(report.depth);
        const input = page.getByRole('combobox', { name: words.usage.chart.splitBy, exact: true });
        await expect(input).toHaveValue(words.usagePerf.choices.splits.model.name);
        await expect(page.getByRole('button', { name: words.usagePerf.help, exact: true })).toHaveCount(1);
        await noTypingIo(page, observed.calls, async () => {
          await input.click();
          const list = page.getByRole('listbox', { name: words.usage.chart.splitBy, exact: true });
          // Acme is an org: ten declared dimensions + Thread + None; Performance adds Topic.
          await expect(list.getByRole('option')).toHaveCount(view === 'usage' ? 12 : 13);
          await expect(list.getByRole('option', { name: words.usagePerf.choices.splits.thread.name, exact: true })).toBeVisible();
          await expect(list.getByRole('option', { name: words.usagePerf.choices.splits.model.name, exact: true })).toHaveAttribute('aria-selected', 'true');
          await input.pressSequentially('program');
          const programme = list.getByRole('option', { name: words.usagePerf.choices.splits.program.name, exact: true });
          await expect(programme).toHaveAccessibleDescription(words.usagePerf.choices.splits.program.description);
          await expect(list.getByRole('option')).toHaveCount(1);
          await capture(page, info, `${view}-split-${mode.locale}-${mode.theme}-1440`, DESKTOP.width, list);
          await input.press('Escape');
          await expect(input).toHaveValue(words.usagePerf.choices.splits.model.name);
        });
        const selected = usageResponse('program');
        await input.click(); await input.pressSequentially('program'); await input.press('Enter');
        const changed = await selected;
        expect(changed.status()).toBe(200);
        const query = new URL(changed.url()).searchParams;
        expect(query.get('scope')).toBe(scope);
        expect(query.get('series_by')).toBe('program');
        expect(query.get('from')).toBe(report.from); expect(query.get('to')).toBe(report.to);
        await expect.poll(() => new URL(page.url()).searchParams.get('series')).toBe('program');
        await expect(input).toHaveValue(words.usagePerf.choices.splits.program.name);
        await expect(page.getByRole('button', { name: words.usagePerf.help, exact: true })).toHaveCount(1);
      }

      const recordsResponse = (outcome: string | null) => page.waitForResponse((response) => {
        const url = new URL(response.url());
        return response.request().method() === 'GET' && url.pathname === '/api/v1/usage/events' && url.searchParams.get('scope') === scope && url.searchParams.get('outcome') === outcome;
      });
      const loaded = recordsResponse('forced');
      await page.goto(`/usage?scope=${encodeURIComponent(scope)}&view=requests&range=7d&outcome=forced`);
      expect((await loaded).status()).toBe(200);
      const input = page.getByRole('combobox', { name: words.usagePerf.outcome, exact: true });
      await expect(input).toHaveValue(words.usagePerf.choices.outcomes.forced.name);
      await noTypingIo(page, observed.calls, async () => {
        await input.click();
        const list = page.getByRole('listbox', { name: words.usagePerf.outcome, exact: true });
        expect(OUTCOMES.length).toBeGreaterThan(10);
        await expect(list.getByRole('option')).toHaveCount(OUTCOMES.length + 1);
        await expect(list.getByRole('option', { name: words.usagePerf.choices.outcomes.forced.name, exact: true })).toHaveAttribute('aria-selected', 'true');
        await input.pressSequentially('cap_reached');
        await expect(list.getByRole('option')).toHaveCount(1);
        await expect(list.getByRole('option', { name: words.usagePerf.choices.outcomes.cap_reached.name, exact: true })).toHaveAccessibleDescription(words.usagePerf.choices.outcomes.cap_reached.description);
        await capture(page, info, `outcome-${mode.locale}-${mode.theme}-1440`, DESKTOP.width, list);
      });
      const selected = recordsResponse('cap_reached');
      await input.press('Enter');
      const filtered = await selected;
      expect(filtered.status()).toBe(200);
      const query = new URL(filtered.url()).searchParams;
      expect(query.get('outcome')).toBe('cap_reached');
      expect(query.get('requests_only')).toBe('true'); expect(query.get('include_running')).toBe('true');
      // The seeded performance report has no cap-reached request; the option still belongs to the complete enum.
      expect((await filtered.json() as UsageEventPage).items).toEqual([]);
      await expect(input).toHaveValue(words.usagePerf.choices.outcomes.cap_reached.name);
      const cleared = recordsResponse(null);
      await input.click(); await input.fill(words.usagePerf.choices.outcomes.all.name);
      await page.getByRole('option', { name: words.usagePerf.choices.outcomes.all.name, exact: true }).click();
      expect((await cleared).status()).toBe(200);
      await expect.poll(() => new URL(page.url()).searchParams.has('outcome')).toBe(false);
      await expect(input).toHaveValue(words.usagePerf.choices.outcomes.all.name);
      expect(observed.calls.filter((call) => call.path.startsWith('/api/v1/usage') && call.method !== 'GET')).toEqual([]);
    } finally { observed.stop(); await recordCalls(info, observed.calls); }
  });

  test(`Trace keeps an unknown current action and applies only a selected native action (${mode.locale}/${mode.theme})`, async ({ page }, info) => {
    await prepare(page, mode);
    const words = mode.locale === 'en' ? english : chinese;
    const path = `/api/v1/nodes/${ACME.org}/audit`;
    const observed = observeApi(page);
    const readAction = (action: string | null) => page.waitForResponse((response) => {
      const url = new URL(response.url());
      return response.request().method() === 'GET' && url.pathname === path && url.searchParams.get('action') === action;
    });
    try {
      const loaded = readAction('future.reindexed');
      await page.goto('/org/acme/settings/audit?action=future.reindexed');
      expect((await loaded).status()).toBe(200);
      const input = page.getByRole('combobox', { name: words.trace.action, exact: true });
      if (!await input.isVisible()) await page.locator('summary').filter({ hasText: words.trace.filters }).click();
      await expect(input).toHaveValue(words.trace.unrecognizedAct);
      const actions = Object.values(ACTIONS).flat();
      expect(actions.indexOf('permission.removed')).toBeGreaterThan(10);
      const label = words.trace.actionVerbs.removed.replace('{{thing}}', words.trace.actionThings.permission);
      await noTypingIo(page, observed.calls, async () => {
        await input.click();
        const list = page.getByRole('listbox', { name: words.trace.action, exact: true });
        await expect(list.getByRole('option')).toHaveCount(actions.length + 2); // All + the unknown current pin.
        await expect(list.getByRole('option', { name: words.trace.unrecognizedAct, exact: true })).toHaveAttribute('aria-selected', 'true');
        await input.pressSequentially('does-not-exist'); await expect(list.getByRole('option')).toHaveCount(0);
        await input.press('Enter'); await input.press('Escape'); await input.press('Enter');
        await expect(input).toHaveValue(words.trace.unrecognizedAct);
        await expect(input).toBeFocused();
        await input.click(); await input.pressSequentially('permission.removed');
        await expect(list.getByRole('option')).toHaveCount(1);
        await expect(list.getByRole('option', { name: label, exact: true })).toBeVisible();
        await capture(page, info, `trace-action-${mode.locale}-${mode.theme}-1440`, DESKTOP.width, list);
      });
      const selected = readAction('permission.removed');
      await input.press('Enter');
      const filtered = await selected;
      expect(filtered.status()).toBe(200);
      expect(new URL(filtered.url()).searchParams.get('action')).toBe('permission.removed');
      await expect(input).toHaveValue(label);
      await expect.poll(() => new URL(page.url()).searchParams.get('action')).toBe('permission.removed');
      const reloaded = readAction('permission.removed'); await page.reload(); expect((await reloaded).status()).toBe(200);
      if (!await input.isVisible()) await page.locator('summary').filter({ hasText: words.trace.filters }).click();
      await expect(input).toHaveValue(label);
      const cleared = readAction(null);
      await input.click(); await input.fill(words.trace.allActions);
      await page.getByRole('option', { name: words.trace.allActions, exact: true }).click();
      expect((await cleared).status()).toBe(200);
      await expect(input).toHaveValue(words.trace.allActions);
      await expect.poll(() => new URL(page.url()).searchParams.has('action')).toBe(false);
      expect(observed.calls.filter((call) => call.path === path && call.method !== 'GET')).toEqual([]);
    } finally { observed.stop(); await recordCalls(info, observed.calls); }
  });
}
