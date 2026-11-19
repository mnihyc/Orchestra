/** The frame says each name once, exposes parents and actions, and keeps its keyboard paths at phone and desktop widths. */
import { expect, type Locator, type Page, type Request, test } from '@playwright/test';
import { ACME, changeTree, scenario, signIn, screenAddress } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import type { FirstScreenPlan } from '../../build/first-wave.ts';
import { slowEdge } from './slow-edge.ts';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

async function minimumTarget(link: Locator) {
  const box = await link.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.width).toBeGreaterThanOrEqual(24);
  expect(box!.height).toBeGreaterThanOrEqual(24);
  expect(await link.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
}

test.beforeEach(async () => { await scenario(); });

async function scriptExchanges(page: Page) {
  return page.evaluate(() => {
    const plan = (window as Window & { __orchStartupPlan?: FirstScreenPlan }).__orchStartupPlan;
    if (plan === undefined) throw new Error('The production HTML must name its initial graph.');
    const selected = plan.pages.find((item) => new RegExp(item.pattern).test(location.pathname));
    if (selected === undefined) throw new Error('The thread component must be in the production preload plan.');
    const required = [...new Set([...plan.common, ...selected.files])];
    const deferred = required.filter((file) => !plan.entry.includes(file));
    const preloaded = new Set([...document.querySelectorAll<HTMLLinkElement>('link[rel="modulepreload"]')].map((link) => new URL(link.href).pathname.slice(1)));
    const resources = performance.getEntriesByType('resource') as PerformanceResourceTiming[];
    const path = (entry: PerformanceResourceTiming) => new URL(entry.name).pathname;
    const scripts = resources.filter((entry) => path(entry).endsWith('.js'));
    const session = resources.find((entry) => path(entry) === '/api/v1/auth/session');
    const bootstrap = resources.find((entry) => path(entry) === '/api/v1/me/bootstrap');
    const late = scripts.filter((entry) => deferred.includes(path(entry).slice(1)));
    const firstPaintAt = performance.getEntriesByName('orch/start/shell-preview-painted', 'mark')[0]?.startTime;
    const firstWave = resources.filter((entry) => [...plan.entry, ...plan.entryStyles].includes(path(entry).slice(1)));
    return { entry: plan.entry, deferred, missing: required.filter((file) => !preloaded.has(file)),
      entryGzipBytes: plan.entryGzipBytes, entryWireBytes: firstWave.reduce((sum, entry) => sum + entry.encodedBodySize, 0), firstPaintAt,
      applicationFile: plan.application?.file,
      pageContentAt: performance.getEntriesByName('orch/page/content', 'mark').at(-1)?.startTime,
      unplanned: scripts.map((entry) => path(entry).slice(1)).filter((file) => !required.includes(file)),
      sessionAt: session?.startTime, bootstrapAt: bootstrap?.startTime,
      firstScriptAt: Math.min(...scripts.map((entry) => entry.startTime)),
      firstDeferredAt: Math.min(...late.map((entry) => entry.startTime)),
      protocol: (performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming).nextHopProtocol };
  });
}

test('thread startup preloads every critical script in two reserved HTTP/1.1 exchanges', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator(`[data-page-content="ready"][data-page-address^="/thread/${ACME.mainThread}"]`)).toBeVisible();
  const proof = await scriptExchanges(page);
  process.stdout.write(`Thread startup exchanges ${JSON.stringify(proof)}\n`);
  expect(proof.missing).toEqual([]); expect(proof.unplanned).toEqual([]);
  // Six HTTP/1.1 slots: reserve one for the HTML session in the first exchange,
  // then two for the authenticated baseline and words in the second exchange.
  expect(proof.entry.length).toBeLessThanOrEqual(5);
  expect(proof.deferred.length).toBeLessThanOrEqual(4);
  expect(proof.entryGzipBytes).toBeLessThanOrEqual(150 * 1024);
  expect(proof.firstPaintAt).toBeDefined();
  expect(proof.firstPaintAt!).toBeLessThanOrEqual(proof.firstDeferredAt);
  expect(proof.sessionAt).toBeDefined(); expect(proof.bootstrapAt).toBeDefined();
  expect(proof.sessionAt!).toBeLessThan(proof.firstScriptAt);
  if (proof.protocol === 'http/1.1') expect(proof.bootstrapAt!).toBeLessThanOrEqual(proof.firstDeferredAt);
});

test('thread startup finishes its scripts within two actual 700 ms HTTP/1.1 exchanges', async ({ page, baseURL }) => {
  if (baseURL === undefined) throw new Error('The latency case requires the owned fake-API test server.');
  await signIn(page);
  const edge = await slowEdge(baseURL, 700);
  try {
    await page.goto(`${edge.origin}/thread/${ACME.mainThread}`);
    await expect(page.locator(`[data-page-content="ready"][data-page-address^="/thread/${ACME.mainThread}"]`)).toBeVisible();
    const proof = await scriptExchanges(page);
    const waves = edge.waves([...proof.entry, ...proof.deferred]);
    process.stdout.write(`Long-line script exchanges ${JSON.stringify({ waves, protocol: proof.protocol, missing: proof.missing, unplanned: proof.unplanned,
      applicationFile: proof.applicationFile, entryGzipBytes: proof.entryGzipBytes, entryWireBytes: proof.entryWireBytes,
      firstPaintAt: proof.firstPaintAt, pageContentAt: proof.pageContentAt })}\n`);
    expect(proof.protocol).toBe('http/1.1');
    expect(proof.missing).toEqual([]); expect(proof.unplanned).toEqual([]);
    expect(new Set(waves.map((item) => item.file.slice(1)))).toEqual(new Set([...proof.entry, ...proof.deferred]));
    expect(Math.max(...waves.map((item) => item.wave))).toBeLessThanOrEqual(2);
    expect(proof.entryGzipBytes).toBeLessThanOrEqual(150 * 1024);
    expect(proof.entryWireBytes).toBeGreaterThan(0);
    expect(proof.entryWireBytes).toBeLessThanOrEqual(150 * 1024);
    expect(proof.firstPaintAt).toBeDefined();
    expect(proof.firstPaintAt!).toBeLessThanOrEqual(6_000);
    expect(proof.firstPaintAt!).toBeLessThanOrEqual(proof.firstDeferredAt);
    expect(proof.sessionAt!).toBeLessThan(proof.firstScriptAt);
    expect(proof.bootstrapAt!).toBeLessThanOrEqual(proof.firstDeferredAt);
  } finally { await edge.close(); }
});

for (const width of [400, 1440]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`startup keeps one baseline and a bare list address at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    await page.waitForLoadState('networkidle');
    const delayed = new Set<ReturnType<typeof setTimeout>>();
    let admissions = 0, reads = 0;
    interface Read { at: number; status?: number; revision?: unknown; failure?: string }
    let observed = { started: Date.now(), reads: [] as Read[], frames: [] as Record<string, unknown>[], writes: [] as Record<string, unknown>[] };
    const requests = new WeakMap<Request, Read>();
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path.endsWith('/me/bootstrap')) {
        reads += 1;
        const read = { at: Date.now() - observed.started }; requests.set(request, read); observed.reads.push(read);
      } else if (request.method() !== 'GET' && path.startsWith('/api/v1/')) {
        observed.writes.push({ at: Date.now() - observed.started, method: request.method(), path });
      }
    });
    page.on('requestfailed', (request) => { const read = requests.get(request); if (read !== undefined) read.failure = request.failure()?.errorText; });
    page.on('response', (response) => {
      const read = requests.get(response.request()); if (read === undefined) return;
      read.status = response.status();
      void response.json().then((body: unknown) => {
        if (typeof body === 'object' && body !== null && 'personal_revision' in body) read.revision = body.personal_revision;
      }).catch(() => undefined);
    });
    await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (route) => {
      const server = route.connectToServer();
      server.onMessage((message) => {
        const frame = JSON.parse(String(message)) as { op?: unknown; feed?: unknown; mode?: unknown; personal_revision?: unknown; data?: { what?: unknown } };
        if (frame.op === 'ready' || frame.feed === 'user') observed.frames.push({ at: Date.now() - observed.started, op: frame.op, mode: frame.mode, revision: frame.personal_revision, what: frame.data?.what });
        if (frame.op === 'ready') {
          // Authentication work exceeds the slow-link threshold without delaying ping replies.
          const timer = setTimeout(() => { delayed.delete(timer); route.send(message); }, 259);
          delayed.add(timer);
        } else {
          if (frame.op === 'subscribed' && frame.feed === 'user' && frame.mode !== 'reload') admissions += 1;
          route.send(message);
        }
      });
    });
    try {
      for (const reload of [false, true]) {
        reads = 0; const before = admissions;
        observed = { started: Date.now(), reads: [], frames: [], writes: [] };
        if (reload) await page.reload(); else await page.goto('/org/acme');
        await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
        await expect.poll(() => admissions).toBeGreaterThan(before);
        await expect(page.locator('html')).toHaveAttribute('lang', locale);
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
        // Cover the former 1.5 s background delay and its 1.5 s idle deadline.
        await page.waitForTimeout(4_000);
        expect(reads, JSON.stringify({ reload, ...observed })).toBe(1);
        expect(new URL(page.url()).searchParams.has('list')).toBe(false);
        expect(new URL(page.url()).searchParams.has('_list')).toBe(false);
        if (width === 1440) {
          await expect(page.getByRole('contentinfo')).toContainText(locale === 'en' ? 'Connected' : '已连接');
          await expect(page.getByRole('contentinfo')).not.toContainText(locale === 'en' ? 'Connection slower' : '连接变慢');
        }
      }
      await page.screenshot({ path: test.info().outputPath('startup.png') });
    } finally { for (const timer of delayed) clearTimeout(timer); }
  });
  test(`single chrome names and full parent targets at ${width}/${locale}/${theme}`, async ({ page }) => {
    const words = locale === 'en' ? { navigation: 'Navigation', home: 'Open Acme home', details: 'Details', main: 'Main area', documents: 'Documents', team: 'Team', help: 'Help' }
      : { navigation: '导航', home: '打开Acme主页', details: '详情', main: '主区域', documents: '文档', team: '团队', help: '帮助' };
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await language(page, locale);
    await page.goto('/org/acme');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toHaveCount(1);
    await expect(page.locator('[data-screen-path]')).toHaveCount(0);
    const main = page.locator('[data-main-area]');
    // The org's page actions are visible buttons, not a primary action hidden in an ellipsis.
    await expect(main.locator('[data-page-actions]').first()).toBeVisible();
    expect(await main.locator('[data-page-actions]').first().locator('button').count()).toBeGreaterThan(1);
    if (width === 1440) {
      await expect(page.getByRole('tablist', { name: words.main, exact: true }).getByRole('tab', { name: 'Acme', exact: true })).toBeVisible();
      const left = page.locator('[data-workbench-bar="left"]');
      await expect(left.locator('[data-container-title]')).not.toContainText('Acme');
      await expect(left.getByRole('button', { name: /^(Team chat|团队聊天)$/ })).toHaveCount(1);
      await expect(page.getByRole('navigation', { name: /^(Orgs and your own places|组织和个人入口)$/ }).getByRole('link', { name: words.help, exact: true })).toHaveCount(1);
      await expect(page.getByRole('contentinfo').getByRole('button', { name: words.help, exact: true })).toHaveCount(0);
    } else {
      await page.getByRole('button', { name: words.navigation, exact: true }).click();
      const drawer = page.getByRole('dialog', { name: words.navigation, exact: true });
      await expect(page.locator('[data-narrow-containers]').getByRole('button', { name: words.navigation, exact: true })).toBeVisible();
      await expect(drawer.locator('[data-container-title]').getByText(words.navigation, { exact: true })).toHaveCount(0);
      const orgHome = drawer.getByRole('link', { name: words.home, exact: true });
      await expect(orgHome).toHaveAttribute('href', '/org/acme');
      await orgHome.click();
      await expect(drawer).toHaveCount(0);
      await page.goto('/settings/profile');
      await page.getByRole('button', { name: words.navigation, exact: true }).click();
      await page.getByRole('dialog', { name: words.navigation, exact: true }).getByRole('link', { name: words.home, exact: true }).click();
      await expect(page).toHaveURL(screenAddress('/org/acme'));
    }

    await page.goto(`/topic/${ACME.fixLogin}?tab=documents`);
    await expect(page.getByRole('tab', { name: words.documents, exact: true })).toHaveCount(1);
    await expect(page.getByRole('tab', { name: words.documents, exact: true })).toHaveAttribute('aria-selected', 'true');
    // A kept header may remain beside the full page; its duplicate body is closed.
    if (width === 1440) await expect(page.locator('[data-workbench-bar="right"] [data-section-view="documents"] [id^="section-body-"]')).toBeHidden();

    await changeTree([{ id: ACME.fixLogin, name: 'Payment routing and sign-in' }]);
    await page.goto(`/thread/${ACME.mainThread}`);
    const trail = page.locator('[data-screen-path]');
    const parent = trail.getByRole('link', { name: 'Payment routing and sign-in', exact: true });
    await expect(parent).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=overview`);
    await minimumTarget(parent);
    await expect(trail.getByRole('link', { name: 'Main thread', exact: true })).toHaveCount(0);
    await expect(trail.locator('[aria-current="page"]')).toHaveCount(0);
    await expect(trail.getByRole('button')).toHaveCount(0);
    if (width === 400) await expect(trail.locator('a:visible')).toHaveCount(1);
    else {
      for (const link of await trail.locator('a:visible').all()) await minimumTarget(link);
      const selected = page.locator('[role="treeitem"][aria-selected="true"]').first();
      await expect(selected).toBeVisible();
      expect(await selected.evaluate((element) => getComputedStyle(element).boxShadow)).toContain('2px');
      for (const link of await selected.locator('a[aria-current="page"]').all()) {
        expect(await link.evaluate((element) => getComputedStyle(element).boxShadow)).toBe('none');
      }
      // An arrow resizes the bar; Home now collapses it at its minimum.
      const right = page.locator('[data-workbench-bar="right"]');
      const before = await right.boundingBox();
      expect(before).not.toBeNull();
      await page.getByRole('separator', { name: /^(Width of the right bar|右栏的宽度)$/ }).press('ArrowLeft');
      await expect.poll(async () => (await right.boundingBox())?.width ?? 0).toBeGreaterThan(before!.width);
      await expect(right.getByRole('tab', { name: words.team, exact: true })).toHaveAttribute('aria-selected', 'true');
    }
    await parent.click();
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}`));
  });
}

test('the rail arrows pass the plus without opening it, and only activation opens its menu', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await signIn(page);
  await page.goto('/settings/profile');
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
  const plus = rail.locator('[data-org-add]');
  await plus.focus();
  await plus.press('ArrowUp');
  await page.keyboard.press('ArrowDown');
  await expect(plus).toBeFocused();
  await expect(page.getByRole('menu')).toHaveCount(0);
  await page.keyboard.press('ArrowDown');
  await expect(rail.getByRole('button', { name: 'Find anything', exact: true })).toBeFocused();
  await expect(page.getByRole('menu')).toHaveCount(0);
  for (const key of ['Enter', 'Space', 'ArrowRight']) {
    await plus.focus();
    await plus.press(key);
    await expect(page.getByRole('menu')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(plus).toBeFocused();
  }
});

test('the first Tab skips navigation and F6 cycles visible areas in both directions', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Message to the agent', exact: true })).toBeVisible();
  await page.waitForLoadState('networkidle');
  await page.evaluate(() => { document.body.tabIndex = -1; document.body.focus(); document.body.removeAttribute('tabindex'); });
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'Skip to main content', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-main-area]')).toBeFocused();
  await page.keyboard.press('F6');
  expect(await page.evaluate(() => document.activeElement?.closest('[data-workbench-bar="right"]') !== null)).toBe(true);
  await page.keyboard.press('Shift+F6');
  expect(await page.evaluate(() => document.activeElement?.closest('[data-view-group]') !== null)).toBe(true);
});

for (const width of [400, 1100, 1440]) {
  test(`unseen machine views subscribe only on reveal and unsubscribe on collapse at ${width}px`, async ({ page }) => {
    const frames: { op: string; id?: number; feed?: string }[] = [];
    page.on('websocket', (socket) => socket.on('framesent', ({ payload }) => {
      let value: unknown;
      try { value = JSON.parse(payload.toString()); } catch { return; } // Other sockets can use binary frames.
      if (typeof value === 'object' && value !== null && 'op' in value && typeof value.op === 'string') {
        frames.push({ op: value.op, id: 'id' in value && typeof value.id === 'number' ? value.id : undefined, feed: 'feed' in value && typeof value.feed === 'string' ? value.feed : undefined });
      }
    }));
    await page.setViewportSize({ width, height: 1000 });
    await signIn(page);
    if (width === 400) {
      // Main tabs persist across navigation. Close the landing org so this
      // phone case measures one conversation rather than its retained org page.
      const landing = page.getByRole('tab', { name: 'Acme', exact: true });
      await landing.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
      await expect(landing).toHaveCount(0);
      await page.waitForLoadState('networkidle');
    }
    frames.length = 0;
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
    await expect.poll(() => frames.some((frame) => frame.op === 'sub' && frame.feed === `thread:${ACME.mainThread}`)).toBe(true);
    if (width === 400) expect(frames.filter((frame) => frame.op === 'sub' && frame.feed?.startsWith('org:') === true)).toEqual([]);
    else {
      await expect.poll(() => frames.some((frame) => frame.op === 'sub' && frame.feed === `org:${ACME.org}`)).toBe(true);
      expect([...new Set(frames.filter((frame) => frame.op === 'sub' && frame.feed?.startsWith('org:') === true).map((frame) => frame.feed))]).toEqual([`org:${ACME.org}`]);
    }
    // The visible conversation rewrites local addresses through its authorized Ports door.
    await expect.poll(() => frames.filter((frame) => frame.op === 'sub' && frame.feed === `env:${RUNS_ON.workspace}:ports`).length).toBe(1);
    const unseen = /^env:[^:]+:(procs|stats|browsers|displays|dir:)/;
    expect(frames.filter((frame) => frame.op === 'sub' && unseen.test(frame.feed ?? ''))).toEqual([]);
    if (width === 400) {
      await page.getByRole('textbox', { name: 'Message to the agent', exact: true }).fill('Kept behind the sheet');
      await showWorkbenchView(page, 'Processes');
      await expect(page.locator('[data-main-area]')).toHaveAttribute('inert', '');
      await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toHaveCount(0);
    } else await showWorkbenchView(page, 'Processes');
    await expect(page.getByTestId('processes-panel')).toBeVisible();
    await expect.poll(() => frames.some((frame) => frame.op === 'sub' && frame.feed?.endsWith(':procs') === true)).toBe(true);
    const subscription = frames.findLast((frame) => frame.op === 'sub' && frame.feed?.endsWith(':procs') === true);
    expect(subscription?.id).toBeDefined();
    const right = page.locator('[data-workbench-bar="right"]');
    const section = right.getByRole('button', { name: 'Processes', exact: true }).and(right.locator('[aria-controls^="section-body-"]'));
    // A single view closes with its container; kept sections retain their own collapse control.
    if (await section.isVisible()) await section.click();
    else await right.getByRole('button', { name: /^(Close|Hide the right bar)$/, exact: true }).click();
    // Leave the edge so the intentionally hover-revealed floating bar closes too.
    await page.mouse.move(0, 0);
    await expect(page.getByTestId('processes-panel')).toBeHidden();
    await expect.poll(() => frames.some((frame) => frame.op === 'unsub' && frame.id === subscription?.id)).toBe(true);
    if (width === 400) {
      const close = right.getByRole('button', { name: 'Close', exact: true });
      if (await close.isVisible()) await close.click();
      await expect(page.getByRole('textbox', { name: 'Message to the agent', exact: true })).toHaveValue('Kept behind the sheet');
      await expect(page.locator('[data-main-area]')).not.toHaveAttribute('inert', '');
    }
  });
}

test('terminal creation and scope help share the tab row, and all-tabs centres on the first text line', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Terminal');
  const bottom = page.locator('[data-view-group]').filter({ has: page.getByRole('tablist', { name: 'Bottom', exact: true }) });
  const create = bottom.locator('[data-view-actions="terminal"]').getByRole('button', { name: 'Open a new shell', exact: true });
  await expect(create).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open a new shell', exact: true })).toHaveCount(1);
  const note = bottom.locator('[data-terminal-note]');
  await expect(note).toHaveCount(0);
  const scope = bottom.getByRole('button', { name: 'Help: Shell access', exact: true });
  await expect(scope).toBeVisible(); await scope.focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog', { name: 'Shell access', exact: true })).toContainText('the whole environment');
  await page.keyboard.press('Escape');
  const centres = await bottom.evaluate((group) => {
    const tab = group.querySelector('[role="tab"]')!;
    const text = Array.from(tab.querySelectorAll('span')).find((span) => span.textContent === 'Terminal')!;
    const range = document.createRange(); range.selectNodeContents(text);
    const first = range.getClientRects()[0]!;
    const button = Array.from(group.querySelectorAll('button')).find((item) => item.getAttribute('aria-label') === 'Find tab in Bottom')!;
    const icon = button.querySelector('svg')!.getBoundingClientRect();
    return { text: first.y + first.height / 2, icon: icon.y + icon.height / 2 };
  });
  expect(Math.abs(centres.text - centres.icon)).toBeLessThan(1);
});
