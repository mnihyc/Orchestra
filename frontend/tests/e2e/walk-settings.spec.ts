/** Settings paths retain their choices and link empty imports to exports in both languages. */
import { expect, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { BindingOptions, NotificationPrefs } from '../../src/generated/api-types';
import { inventoryRow } from '../doubles/everything-row';
import { id, RUNS_ON } from '../doubles/fixtures';
import { installImportFixture } from './import-support';
import { approvalsMade, askAccess, inboxHeld, notify } from './inbox-support';
import { ACME, replySent, scenario, signIn } from './support';
import { requestsOf } from './start-support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Thread → pending continuation from Opening → Cancel → no future line; no topic-list read.
  // 线程 → 开场响应中的待继续项 → 取消 → 待继续项消失；不读取整个主题的计划列表。
  test(`thread opening supplies pending continuation and cancellation ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale); await page.goto('about:blank');
    const [native] = await Promise.all([
      page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/threads/${ACME.mainThread}/opening`)
        .then(replySent),
      page.goto(`/thread/${ACME.mainThread}`),
    ]);
    if (native === null || typeof native !== 'object' || Array.isArray(native)
      || !('pending_agent_schedules' in native)) throw new Error('Expected the native opening projection');
    const part = native.pending_agent_schedules;
    if (part === null || typeof part !== 'object' || !('data' in part) || part.data === null
      || typeof part.data !== 'object' || !('seq' in part.data) || typeof part.data.seq !== 'number') throw new Error('Expected its native topic cut');
    const seq = part.data.seq;
    // Retire the preparation document so the measured cold navigation owns fresh caches.
    await page.goto('about:blank');
    const words = locale === 'en' ? english : chinese, schedule = id(0x78, 1);
    const reads: string[] = [], traffic: string[] = [], cancellations: string[] = [];
    let cancelled = false;
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') {
        traffic.push(`${request.method()} ${url.pathname}${url.search}`);
        if (request.method() === 'GET') reads.push(url.pathname + url.search);
      }
    });
    // The backend deciding case exercises native scheduling/admission; here the browser consumes
    // exactly its public four-field projection, without installing a full topic snapshot.
    await page.route(`**/threads/${ACME.mainThread}/opening?**`, async (route) => {
      await route.fulfill({ status: 200, json: { ...native, pending_agent_schedules: { status: 'ready', data: {
        seq, items: cancelled ? [] : [{ id: schedule, name: 'Check the CI run', state: 'active', next_run_at: '2030-01-01T21:30:00Z' }],
      } } } });
    });
    await page.route(`**/input-schedules/${schedule}`, async (route) => {
      if (route.request().method() !== 'DELETE') { await route.continue(); return; }
      cancellations.push(route.request().method()); cancelled = true; await route.fulfill({ status: 204 });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    const pending = page.getByRole('list', { name: words.agentSchedules.pending, exact: true });
    await expect(pending.getByText('Check the CI run', { exact: true })).toBeVisible();
    await expect(pending.getByText(words.agentSchedules.continueAt, { exact: true })).toBeVisible();
    await page.waitForLoadState('networkidle');
    expect(reads.filter((path) => path.startsWith(`/api/v1/threads/${ACME.mainThread}/opening?`))).toHaveLength(1);
    expect(reads.filter((path) => path.startsWith(`/api/v1/topics/${ACME.fixLogin}/sync`))).toEqual([]);
    expect(traffic.length, traffic.join('\n')).toBeLessThanOrEqual(8);
    await info.attach('native-pending-continuation', { body: await page.screenshot(), contentType: 'image/png' });
    const refreshed = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.pathname === `/api/v1/threads/${ACME.mainThread}/opening` && url.searchParams.get('limit') === '1';
    });
    await pending.getByRole('button', { name: words.common.cancel, exact: true }).click();
    await expect(pending).toHaveCount(0); expect((await refreshed).ok()).toBe(true); await page.waitForLoadState('networkidle');
    expect(cancellations).toEqual(['DELETE']);
    expect(reads.filter((path) => path.startsWith(`/api/v1/topics/${ACME.fixLogin}/sync`))).toEqual([]);
    await info.attach('pending-continuation-traffic', { body: JSON.stringify({ reads, traffic, cancellations }), contentType: 'application/json' });
  });

  test(`MCP destination choices read when opened ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    expect((await page.request.post(`${DOUBLE}/__double/request-feeds/fixtures`)).ok()).toBe(true);
    await signIn(page); await language(page, locale); await page.goto('about:blank');
    const words = locale === 'en' ? english : chinese;
    const trees: string[] = [];
    page.on('request', (request) => { const path = new URL(request.url()).pathname; if (/\/orgs\/[^/]+\/tree$/.test(path)) trees.push(path); });
    await page.goto(`/resources/mcp/${id(0x31, 1)}`);
    const picker = page.getByRole('combobox', { name: words.mcpSwitch.place, exact: true });
    await expect(picker).toBeVisible(); await page.waitForLoadState('networkidle');
    const acme = `/api/v1/orgs/${ACME.org}/tree`, first = trees.filter((path) => path === acme).length;
    expect(first).toBeLessThanOrEqual(1);
    await picker.click();
    const path = 'Acme / Research / Web / Fix login';
    await page.getByRole('option', { name: path, exact: true }).click();
    await expect(page.getByRole('button', { name: words.settings.place.clear.replace('{{path}}', path), exact: true })).toBeVisible();
    expect(trees.filter((read) => read === acme)).toHaveLength(1);
    await info.attach('mcp-destination-choice', { body: await page.screenshot(), contentType: 'image/png' });
  });

  test(`place frames share their visible overview read ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const reads: URL[] = [];
    page.on('request', (request) => { if (request.method() === 'GET') reads.push(new URL(request.url())); });
    for (const [kind, id, name] of [['project', ACME.web, 'Web'], ['topic', ACME.fixLogin, 'Fix login']] as const) {
      await page.goto('about:blank'); reads.length = 0;
      await page.goto(`/${kind}/${id}`);
      await expect(page.getByRole('heading', { name, level: 1, exact: true })).toBeVisible();
      await page.waitForLoadState('networkidle');
      expect(reads.filter((url) => url.pathname === `/api/v1/nodes/${id}/overview`)).toHaveLength(1);
      expect(reads.filter((url) => url.pathname === `/api/v1/nodes/${id}` || url.pathname === `/api/v1/nodes/${id}/port-openings`
        || url.pathname === '/api/v1/usage' && url.searchParams.get('scope') === `node:${id}`)).toEqual([]);
      await info.attach(`${kind}-native-overview`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });

  test(`failed environment keeps its reason without a default mark ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch(), offered = await response.json() as BindingOptions;
      await route.fulfill({ response, json: { ...offered, environments: offered.environments.map((row, index) => index === 0
        ? { ...row, is_default: true, environment: { ...row.environment, name: 'Failed Main', status: 'failed' } } : { ...row, is_default: false }) } });
    });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.locator('[data-quick-start]').getByRole('button', { name: words.start.quick.change, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
    await dialog.getByRole('radio', { name: words.start.where.useExisting, exact: true }).click();
    await dialog.getByRole('combobox', { name: words.start.where.title, exact: true }).click();
    const main = page.getByRole('option', { name: 'Failed Main', exact: true });
    await expect(main).toHaveAttribute('aria-disabled', 'true');
    await expect(main).toHaveAccessibleDescription(locale === 'en' ? /Unavailable after a failed start/ : /上次启动失败/);
    await expect(main).not.toHaveAccessibleDescription(new RegExp(words.start.topicDefault));
    await info.attach('failed-environment-choice', { body: await page.screenshot(), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
  });

  // Topic → type while metadata loads → one Workspace read → select Sandbox → one new judgment; draft/facts stay.
  // 主题 → 元数据加载时输入草稿 → Workspace 只读一次 → 改选 Sandbox → 只检查新选择一次；草稿和信息保留。
  test(`topic binding reads once and keeps its first paint ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.addInitScript(() => {
      const events: { kind: string; url: string; status?: number }[] = [];
      Object.assign(window, { __bindingTransport: events });
      const send = window.fetch.bind(window);
      window.fetch = (input, init) => {
        const url = input instanceof Request ? input.url : String(input);
        if (!url.includes('/binding/options')) return send(input, init);
        const signal = input instanceof Request ? input.signal : init?.signal;
        events.push({ kind: 'send', url });
        if (signal?.aborted) events.push({ kind: 'abort', url });
        signal?.addEventListener('abort', () => events.push({ kind: 'abort', url }));
        return send(input, init).then((response) => {
          events.push({ kind: 'response', url, status: response.status });
          const stream = response.body;
          if (stream !== null) {
            const getReader = stream.getReader.bind(stream);
            Object.defineProperty(stream, 'getReader', { value: function (this: ReadableStream<Uint8Array>, ...args: unknown[]) {
              const reader = Reflect.apply(getReader, this, args) as ReadableStreamDefaultReader<Uint8Array>;
              const read = reader.read.bind(reader);
              reader.read = async () => {
                try { const chunk = await read(); if (chunk.done) events.push({ kind: 'complete', url }); return chunk; }
                catch (error) { events.push({ kind: 'failed', url }); throw error; }
              };
              const cancel = reader.cancel.bind(reader);
              reader.cancel = (reason?: unknown) => { events.push({ kind: 'cancel', url }); return cancel(reason); };
              return reader;
            } });
          }
          return response;
        }, (error: unknown) => { events.push({ kind: 'failed', url }); throw error; });
      };
    });
    const reads: URL[] = [], failed: string[] = [], metadataReads: URL[] = [], usageReads: URL[] = [], usageFailed: string[] = [];
    let releaseMetadata: () => void = () => undefined, releaseInitial: () => void = () => undefined, releaseChanged: () => void = () => undefined;
    const metadata = new Promise<void>((resolve) => { releaseMetadata = resolve; });
    const initial = new Promise<void>((resolve) => { releaseInitial = resolve; });
    const changed = new Promise<void>((resolve) => { releaseChanged = resolve; });
    let markInitialServed: () => void = () => undefined, markChangedServed: () => void = () => undefined;
    const initialServed = new Promise<void>((resolve) => { markInitialServed = resolve; });
    const changedServed = new Promise<void>((resolve) => { markChangedServed = resolve; });
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname.endsWith(`/nodes/${ACME.fixLogin}/binding/options`)) reads.push(url);
      if (url.pathname.endsWith(`/nodes/${ACME.fixLogin}/overview`)) metadataReads.push(url);
      if (url.pathname === '/api/v1/usage' && url.searchParams.get('scope') === `node:${ACME.fixLogin}`) usageReads.push(url);
    });
    page.on('requestfailed', (request) => {
      const url = new URL(request.url());
      if (url.pathname.endsWith(`/nodes/${ACME.fixLogin}/binding/options`)) failed.push(request.url());
      if (url.pathname === '/api/v1/usage' && url.searchParams.get('scope') === `node:${ACME.fixLogin}`) usageFailed.push(request.url());
    });
    await page.route(`**/nodes/${ACME.fixLogin}/overview**`, async (route) => {
      const response = await route.fetch(); await metadata; await route.fulfill({ response });
    });
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch();
      const selected = new URL(route.request().url()).searchParams.get('environment_id') === RUNS_ON.sandbox;
      await (selected ? changed : initial);
      await route.fulfill({ response });
      (selected ? markChangedServed : markInitialServed)();
    });
    try {
      await page.goto(`/topic/${ACME.fixLogin}`);
      await expect.poll(() => metadataReads.length).toBe(1);
      expect(reads).toHaveLength(0);
      releaseMetadata();
      await expect(page.getByRole('heading', { name: 'Fix login', level: 1, exact: true })).toBeVisible();
      const composer = page.locator('[data-quick-start]');
      await composer.getByRole('textbox').fill('Keep the first paint and this draft');
      await expect.poll(() => reads.length).toBe(1);
      expect(reads[0]?.searchParams.get('environment_id')).toBe(RUNS_ON.workspace);
      const orgValues = await page.locator('input[role="combobox"]').evaluateAll((inputs, placeholder) => inputs
        .filter((input) => (input as HTMLInputElement).placeholder === placeholder)
        .map((input) => (input as HTMLInputElement).value), words.navigation.chooseOrg);
      expect(orgValues).not.toContain('');
      await info.attach('topic-binding-initial', { body: await page.screenshot(), contentType: 'image/png' });
      releaseInitial();
      await initialServed;
      await composer.getByRole('button', { name: words.start.quick.change, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
      await expect(dialog.getByRole('combobox', { name: words.start.agent.title, exact: true })).toHaveValue('Codex');
      await expect(dialog.getByRole('button', { name: words.start.create, exact: true })).toBeEnabled();
      expect(reads).toHaveLength(1);
      await dialog.getByRole('button', { name: words.start.options.title, exact: true }).click();
      const draft = dialog.getByRole('textbox', { name: words.start.message.title, exact: true });
      await expect(draft).toHaveValue('Keep the first paint and this draft');
      await dialog.getByRole('combobox', { name: words.start.where.moreTitle, exact: true }).click();
      await page.getByRole('option', { name: 'Sandbox', exact: true }).click();
      await expect.poll(() => reads.length).toBe(2);
      expect(reads[1]?.searchParams.get('environment_id')).toBe(RUNS_ON.sandbox);
      await expect(dialog.getByRole('combobox', { name: words.start.agent.title, exact: true })).toHaveValue('Codex');
      await expect(draft).toHaveValue('Keep the first paint and this draft');
      await expect(dialog.getByRole('button', { name: words.start.create, exact: true })).toBeDisabled();
      await info.attach('topic-binding-change', { body: await page.screenshot(), contentType: 'image/png' });
      releaseChanged();
      await changedServed;
      await expect(dialog.getByRole('button', { name: words.start.create, exact: true })).toBeEnabled();
      await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
      await expect(composer.getByRole('textbox')).toHaveValue('Keep the first paint and this draft');
      // Chromium can report an intercepted request as aborted after fetch consumed its complete
      // body. Check the application's actual signal, reader and EOF, retaining CDP diagnostics.
      const transport = await page.evaluate(() => (window as Window & {
        __bindingTransport: { kind: string; url: string; status?: number }[];
      }).__bindingTransport);
      await info.attach('binding-transport', { body: JSON.stringify({ transport, cdpFailures: failed }), contentType: 'application/json' });
      expect(reads).toHaveLength(2);
      for (const read of reads) expect(transport.filter((event) => event.url === read.href)).toEqual([
        { kind: 'send', url: read.href }, { kind: 'response', url: read.href, status: 200 }, { kind: 'complete', url: read.href },
      ]);
      expect(metadataReads).toHaveLength(1); expect(usageReads.length).toBeLessThanOrEqual(1); expect(usageFailed).toEqual([]);
      await info.attach('topic-request-counts', { body: JSON.stringify({
        metadata: metadataReads.length, bindingEnvironments: reads.map((url) => url.searchParams.get('environment_id')),
        usage: usageReads.length, bindingAborts: transport.filter((event) => event.kind === 'abort').length,
        bindingCdpFailures: failed.length, usageAborts: usageFailed.length,
      }), contentType: 'application/json' });
    } finally { releaseMetadata(); releaseInitial(); releaseChanged(); }
  });

  test(`machine link samples remain distinct ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    for (const state of ['missing', 'partial', 'zero', 'live'] as const) {
      const rates = state === 'missing' ? {} : state === 'partial' ? { round_trip_ms: 16, up_bps: 0 }
        : state === 'zero' ? { round_trip_ms: 0, up_bps: 0, down_bps: 0 } : { round_trip_ms: 200, up_bps: 8000, down_bps: 16000 };
      const seeded = await request.post(`${DOUBLE}/__double/resources/machine-link`, { data: { machine_id: RUNS_ON.laptop, rates } });
      expect(seeded.ok()).toBe(true);
      await page.goto(`/resources/machines/${RUNS_ON.laptop}`);
      const label = state === 'missing' ? words.machines.overview.linkPending : words.machines.overview.link;
      const value = page.locator('[data-resource-facts] [data-item]').filter({ has: page.locator('dt').filter({ hasText: label }) }).locator('dd');
      const expected = state === 'missing' ? '–' : state === 'partial'
        ? words.machines.overview.linkPartial.replace('{{trip}}', words.machines.overview.linkTrip.replace('{{value}}', '16')).replace('{{up}}', '0 bit/s').replace('{{down}}', '–')
        : words.machines.overview.linkValue.replace('{{trip}}', state === 'zero' ? '0' : '200').replace('{{up}}', state === 'zero' ? '0 bit/s' : '8 kbit/s').replace('{{down}}', state === 'zero' ? '0 bit/s' : '16 kbit/s');
      await expect(value).toContainText(expected);
      if (state === 'missing') {
        await expect(value).toHaveText('–');
        await value.locator('[tabindex="0"]').focus();
        await expect(page.getByRole('tooltip')).toHaveText(words.machines.overview.notMeasured);
        await page.keyboard.press('Escape');
      }
      if (state === 'live') await expect(value.getByText(words.machines.overview.slow, { exact: true })).toBeVisible();
      else await expect(value.getByText(words.machines.overview.slow, { exact: true })).toHaveCount(0);
      await info.attach(`machine-link-${state}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  });

  test(`new thread selects a reported agent in its existing environment ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const judged: (string | null)[] = [];
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch();
      const offered = await response.json() as BindingOptions;
      const environment = new URL(route.request().url()).searchParams.get('environment_id');
      judged.push(environment);
      await route.fulfill({ response, json: { ...offered, environment_default: 'fresh', hosts: [],
        providers: environment !== null ? offered.providers : offered.providers.map((provider) => ({ ...provider,
          agents: provider.agents.map((agent) => ({ ...agent, usable: false, reason: 'no_host',
            arrangements: agent.arrangements.map((arrangement) => ({ ...arrangement, available: false, reason: 'no_host' })),
          })),
        })),
      } });
    });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.locator('[data-quick-start]').getByRole('button', { name: words.start.quick.change, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
    await expect(dialog.getByRole('combobox', { name: words.start.where.title, exact: true })).toHaveValue('Workspace');
    const agent = dialog.getByRole('combobox', { name: words.start.agent.title, exact: true });
    await expect(agent).toHaveValue('Codex');
    await agent.click();
    await expect(page.getByRole('option', { name: 'Codex', exact: true })).not.toHaveAttribute('aria-disabled', 'true');
    await page.keyboard.press('Escape');
    await expect(agent).toHaveValue('Codex');
    expect(judged).toContain(RUNS_ON.workspace);
    await info.attach('reported-agent-selected', { body: await page.screenshot(), contentType: 'image/png' });
    await dialog.getByRole('button', { name: words.start.create, exact: true }).click();
    await expect(page).toHaveURL(/\/thread\//);
    const created = await requestsOf('POST', new RegExp(`/topics/${ACME.fixLogin}/threads$`));
    expect(created).toHaveLength(1);
    expect(created[0]?.body).toMatchObject({ binding: { agent_id: RUNS_ON.codex }, environment: { use: RUNS_ON.workspace } });
  });

  test(`everything keeps one visible name and a real destination ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const supplied = await page.request.post(`${DOUBLE}/__double/everything/rows`, { data: { username: 'ada', rows: [inventoryRow({
      id: ACME.org, kind: 'org', name: 'Acme', place_label: 'acme', href: '/org/acme',
      path: [{ id: ACME.org, kind: 'org', name: 'Acme', redacted: false }],
    })] } });
    expect(supplied.ok()).toBeTruthy();
    await signIn(page); await language(page, locale); await page.goto('/everything');
    const row = page.locator(`[data-everything-row="org:${ACME.org}"]`);
    await expect(row.getByText('Acme', { exact: true })).toHaveCount(1);
    await expect(row.getByText('acme', { exact: true })).toHaveCount(0);
    await info.attach('everything-name', { body: await page.screenshot(), contentType: 'image/png' });
    await row.locator('a[data-open]').click();
    await expect(page).toHaveURL(/\/org\/acme\/?(\?list=[^&]*&_list=true)?$/);
  });

  test(`sign-in recovery navigation with and without mail ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const words = locale === 'en' ? english : chinese;
    for (const mail of [true, false]) {
      await scenario({ meta: { mail } });
      await page.goto('/login'); await page.reload();
      if (locale === 'zh-CN') await page.getByRole('radio', { name: '简体中文', exact: true }).click();
      const recovery = page.getByRole('link', { name: words.auth.links.forgot, exact: true });
      await expect(recovery).toHaveAttribute('href', '/recover');
      await recovery.focus(); await page.keyboard.press('Enter');
      await expect(page).toHaveURL(/\/recover$/);
      if (mail) await expect(page.getByLabel(words.auth.fields.email, { exact: true })).toBeVisible();
      else {
        await expect(page.getByText(words.auth.recover.noMail, { exact: true })).toBeVisible();
        await expect(page.getByRole('button', { name: words.auth.recover.submit, exact: true })).toHaveCount(0);
      }
      await info.attach(`recover-mail-${mail}`, { body: await page.screenshot(), contentType: 'image/png' });
      await page.getByRole('link', { name: words.auth.links.backToSignIn, exact: true }).click();
      await expect(page).toHaveURL(/\/login$/);
    }
  });

  test(`access request batch review ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const batch = '00000000-0000-4000-802a-000000000077';
    const first = await askAccess({ batch_id: batch, requester: 'grace', title: 'Read the plan', permissions: ['thread.view'], expires_at: '2099-01-01T12:00:00Z',
      thread_id: ACME.mainThread, node: { id: ACME.mainThread, kind: 'thread', name: 'Main thread', redacted: false }, object: { id: ACME.mainThread, kind: 'thread', name: 'Main thread' } });
    const second = await askAccess({ batch_id: batch, requester: 'grace', title: 'Read the follow-up', permissions: ['thread.view'],
      thread_id: ACME.idleThread, node: { id: ACME.idleThread, kind: 'thread', name: 'Follow-up', redacted: false }, object: { id: ACME.idleThread, kind: 'thread', name: 'Follow-up' } });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto('/approvals');
    await page.getByRole('button', { name: words.approvals.batch.open, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.approvals.batch.title, exact: true });
    await dialog.getByRole('textbox', { name: words.approvals.note, exact: true }).fill('Reviewed together');
    const selectSecond = dialog.getByRole('checkbox', { name: words.approvals.batch.selectNamed.replace('{{name}}', second.title), exact: true });
    await selectSecond.uncheck();
    expect((await approvalsMade()).filter((row) => row.state !== 'pending')).toHaveLength(0);
    await dialog.getByRole('button', { name: words.approvals.batch.approve, exact: true }).click();
    await expect.poll(async () => (await approvalsMade()).find((row) => row.id === first.id)?.state).toBe('approved');
    expect((await approvalsMade()).find((row) => row.id === second.id)?.state).toBe('pending');
    expect((await approvalsMade()).find((row) => row.id === first.id)?.decision).toMatchObject({ approve: true, permissions: ['thread.view'], expires_at: '2099-01-01T12:00:00Z' });
    await selectSecond.check();
    await dialog.getByRole('button', { name: words.approvals.batch.deny, exact: true }).click();
    await expect.poll(async () => (await approvalsMade()).find((row) => row.id === second.id)?.state).toBe('denied');
    await expect(dialog.getByRole('button', { name: words.approvals.batch.approve, exact: true })).toBeDisabled();
    await info.attach('access-batch-outcomes', { body: await page.screenshot(), contentType: 'image/png' });
    await dialog.getByRole('contentinfo').getByRole('button', { name: words.common.close, exact: true }).click();
    await page.getByRole('tab', { name: words.approvals.tabs.history, exact: true }).click();
    await expect(page.getByText(first.title, { exact: true })).toBeVisible();
    await expect(page.getByText(second.title, { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  });

  // Inbox → Kind → failed save → Try again → same rows/view → reload keeps Kind.
  // 收件箱 → 类型 → 保存失败 → 重试 → 通知和分组保留 → 刷新后仍按类型分组。
  test(`inbox view save can retry without changing the grouping ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await notify({ kind: 'turn.completed', title: 'Review the saved grouping' });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese, writes: unknown[] = [];
    await page.route('**/me/state/inbox', async (route) => {
      if (route.request().method() !== 'PUT') { await route.continue(); return; }
      writes.push(route.request().postDataJSON());
      if (writes.length === 1) await route.fulfill({ status: 503, json: {
        type: 'orch:error:internal', code: 'internal', title: '', detail: '', status: 503, request_id: '',
      } });
      else await route.continue();
    });
    await page.goto('/inbox');
    const grouping = page.getByRole('combobox', { name: words.away.inbox.grouping });
    await grouping.click();
    await page.getByRole('option').filter({ has: page.getByText(words.choiceCatalog.inboxGrouping.kind.name, { exact: true }) }).click();
    const failure = page.getByRole('alert');
    await expect(failure).toContainText(words.away.inbox.viewSaveFailed);
    await expect(grouping).toContainText(words.choiceCatalog.inboxGrouping.kind.name);
    await expect(page.locator('li[data-kind="turn.completed"]')).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
    await info.attach('inbox-view-save-recovery', { body: await page.screenshot(), contentType: 'image/png' });
    const accepted = page.waitForResponse((response) => response.request().method() === 'PUT'
      && new URL(response.url()).pathname.endsWith('/me/state/inbox') && response.ok());
    await failure.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await (await accepted).finished();
    await expect(failure).toHaveCount(0);
    expect(writes).toHaveLength(2); expect(writes[1]).toEqual(writes[0]);
    await expect(page.locator('li[data-kind="turn.completed"]')).toHaveCount(1);
    await page.reload();
    await expect(page.getByRole('combobox', { name: words.away.inbox.grouping })).toContainText(words.choiceCatalog.inboxGrouping.kind.name);
    await expect(page.locator('li[data-kind="turn.completed"]')).toHaveCount(1);
    await info.attach('inbox-view-save-requests', { body: JSON.stringify(writes), contentType: 'application/json' });
  });

  test(`inbox kind groups and provider destination ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const job = await notify({ kind: 'job.finished', title: 'Scripted model: A job failed', url: `/resources/providers/${RUNS_ON.openAiKey}` });
    await notify({ kind: 'turn.completed', title: 'First completed turn', node: ACME.mainThread, thread_id: ACME.mainThread });
    await notify({ kind: 'turn.completed', title: 'Second completed turn', node: ACME.idleThread, thread_id: ACME.idleThread });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto('/inbox');
    await page.getByRole('combobox', { name: words.away.inbox.grouping, exact: true }).click();
    await page.getByRole('option', { name: words.choiceCatalog.inboxGrouping.kind.name, exact: true }).click();
    await expect(page.getByRole('heading', { name: words.inbox.kinds.turn_completed, level: 2, exact: true })).toHaveCount(1);
    await expect(page.locator('li[data-kind="turn.completed"]')).toHaveCount(2);
    await page.reload();
    await expect(page.getByRole('combobox', { name: words.away.inbox.grouping, exact: true })).toContainText(words.choiceCatalog.inboxGrouping.kind.name);
    const target = page.locator('li[data-kind="job.finished"] a').first();
    await expect(target).toHaveAttribute('href', `/resources/providers/${RUNS_ON.openAiKey}`);
    await info.attach('inbox-kinds', { body: await page.screenshot(), contentType: 'image/png' });
    await target.focus(); await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/resources/providers/${RUNS_ON.openAiKey}$`));
    expect((await inboxHeld()).rows.find((row) => row.id === job.id)?.read_at).not.toBeNull();
  });

  test(`agent parameter descriptions ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const description = 'How much of a conversation Example agent keeps, in tokens';
    const translated = '示例代理保留的对话长度，以词元计';
    // Direct reads and admitted openings consume the same authored native program report.
    const report = await request.post(`${DOUBLE}/__double/resources/report`, { data: {
        agent: RUNS_ON.codexOnLaptop,
        parameters: [{ key: 'context', type: 'integer', title: 'Context', description, default: { from_model: 'default_context' } }],
        text: { 'zh-CN': { 'harness.name': '示例代理', 'parameters.context.title': '上下文', 'parameters.context.description': translated } },
    } });
    expect(report.status()).toBe(204);
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}`);
    await page.getByRole('tab', { name: new RegExp(`^${words.agents.tabs.parameters}`) }).click();
    const row = page.locator('[data-setting-row]').filter({ hasText: locale === 'en' ? description : translated });
    await expect(row).toContainText(locale === 'en' ? 'Context' : '上下文');
    await expect(row.getByRole('link', { name: words.resourceDetails.layout.openDefaults, exact: true })).toHaveAttribute('href', /setting=context/);
    await row.locator('[data-setting-description]').focus();
    await expect(page.getByRole('tooltip')).toContainText(locale === 'en' ? description : translated);
    await page.keyboard.press('Escape');
    await info.attach('agent-context', { body: await page.screenshot(), contentType: 'image/png' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  });

  test(`notification areas and imports links ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await installImportFixture(page);
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    await page.goto('/settings/notifications');
    const content = page.locator('[data-settings-content]');
    for (const name of Object.values(words.notifications.kindAreas)) await expect(content.getByRole('group', { name, exact: true })).toBeVisible();
    await expect(content.getByText(words.navigation.projects, { exact: true })).toHaveCount(0);
    const turn = content.getByRole('group', { name: words.notifications.kindAreas.work, exact: true }).getByRole('checkbox', { name: words.inbox.kinds.turn_completed, exact: true });
    const machine = content.getByRole('group', { name: words.notifications.kindAreas.resources, exact: true }).getByRole('checkbox', { name: words.inbox.kinds.machine_offline, exact: true });
    const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && response.url().endsWith('/me/notification-prefs'));
    await turn.uncheck(); const accepted = await saved; expect(accepted.ok()).toBeTruthy();
    const acknowledgement = await replySent(accepted) as NotificationPrefs;
    expect(acknowledgement.disabled_kinds).toContain('turn.completed');
    expect(acknowledgement.disabled_kinds).not.toContain('machine.offline');
    await expect(turn).not.toBeChecked();
    await expect(machine).toBeChecked();
    await page.reload();
    await expect(turn).not.toBeChecked(); await expect(machine).toBeChecked();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await info.attach('notification-areas', { body: await page.screenshot(), contentType: 'image/png' });
    await page.goto('/settings/imports');
    await expect(content.getByText(words.importWizard.mine.empty, { exact: true })).toBeVisible();
    await content.getByRole('link', { name: words.importWizard.mine.exports, exact: true }).click();
    await expect(page).toHaveURL(/\/settings\/exports$/);
    await page.goBack();
    await content.getByRole('button', { name: words.importWizard.mine.import, exact: true }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await info.attach('import-source', { body: await page.screenshot(), contentType: 'image/png' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  });
}
