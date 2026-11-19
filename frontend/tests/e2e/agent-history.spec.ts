/** Managed history uses the production timeline, setting and build-check surfaces at both widths and themes. */
import { expect, test, type Page, type Request, type TestInfo } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { marker } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, configureThread, emit, scenario, signIn } from './support.ts';
import { asAdmin } from './admin-mode-support.ts';
import { HISTORY_KEY as KEY, PROGRAM_EXIT_REASON, historyBuilds, historySetting, programExitProvider } from './agent-history-support.ts';

const IDLE_MS = 60_000;

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

function traffic(page: Page) {
  const requests: string[] = [];
  const listen = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push(`${request.method()} ${url.pathname}${url.search}`);
  };
  page.on('request', listen);
  return { requests, stop: () => page.off('request', listen) };
}

async function settled(page: Page) {
  await page.waitForLoadState('networkidle');
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => Number.isFinite(animation.effect?.getComputedTiming().endTime))
      .map((animation) => animation.finished.catch(() => undefined)));
  });
  expect(await page.locator('script[src]').evaluateAll((scripts) => scripts.some((script) => script.getAttribute('src')?.startsWith('/src/') === true))).toBe(false);
  await expect(page.locator('body')).not.toContainText('Unexpected Application Error');
}

async function capture(page: Page, info: TestInfo, name: string) {
  await settled(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'The page must stay inside the viewport').toBe(true);
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path, caret: 'hide' });
  await info.attach(name, { path, contentType: 'image/png' });
}

async function idle(page: Page, reads: ReturnType<typeof traffic>, initial: number) {
  await page.clock.install();
  await page.clock.fastForward(IDLE_MS);
  await settled(page);
  return reads.requests.slice(initial);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  const words = locale === 'en' ? english : chinese;
  const suffix = `${locale}-${theme}-${width}`;

  test(`provider program exit stays visible after restart ${suffix}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const fixture = await programExitProvider();
    await fixture.setState('none');
    await page.goto(fixture.path); await settled(page);
    await expect(page.getByText(words.providers.runtime.lastProgramError, { exact: true })).toHaveCount(0);
    await fixture.setState('error');
    await expect(page.getByText(PROGRAM_EXIT_REASON, { exact: true }).first()).toBeVisible(); await settled(page);
    const reads = traffic(page);
    try {
      await page.reload(); await expect(page.getByText(PROGRAM_EXIT_REASON, { exact: true }).first()).toBeVisible(); await settled(page);
      const cold = [...reads.requests];
      await page.getByText(words.providers.runtime.lastProgramError, { exact: true }).scrollIntoViewIfNeeded();
      await capture(page, info, `provider-error-${suffix}`);
      const idleReads = await idle(page, reads, cold.length);
      await page.clock.resume();
      const beforeAction = reads.requests.length;
      await fixture.setState('online');
      await page.reload(); await settled(page);
      await expect(page.getByText(words.providers.runtime.harnessStatus.online, { exact: true }).first()).toBeVisible();
      await expect(page.getByRole('link', { name: fixture.source.name, exact: true }).first()).toHaveAttribute('href', `/resources/machines/${fixture.source.id}`);
      await expect(page.getByText(PROGRAM_EXIT_REASON, { exact: true }).first()).toBeVisible();
      await page.getByText(words.providers.runtime.lastProgramError, { exact: true }).scrollIntoViewIfNeeded();
      await capture(page, info, `provider-restarted-${suffix}`);
      const action = reads.requests.slice(beforeAction);
      const beforeWarm = reads.requests.length;
      await fixture.setState('unknown');
      await page.reload(); await settled(page);
      await expect(page.getByText(words.providers.runtime.lastProgramError, { exact: true })).toBeVisible();
      await expect(page.getByText(PROGRAM_EXIT_REASON, { exact: true })).toHaveCount(0);
      await page.getByText(words.providers.runtime.lastProgramError, { exact: true }).scrollIntoViewIfNeeded();
      await capture(page, info, `provider-unknown-reason-${suffix}`);
      await info.attach('requests', { body: JSON.stringify({ cold, idle: idleReads, action, warm: reads.requests.slice(beforeWarm) }, null, 2), contentType: 'application/json' });
    } finally { reads.stop(); }
  });

  test(`history lines and person-card reads ${suffix}`, async ({ page }, info) => {
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    const data = { segment_id: 'old-history-segment', program: 'Codex', bytes: 4 * 1024 ** 3, removed_at: '2026-10-20T12:00:00Z', by: ADA, recovered: null } as const;
    await emit(ACME.mainThread, { thread: { state: 'idle' }, items: [
      { ...marker('history-removed', { marker: 'agent_history', data }), created_at: data.removed_at },
      { ...marker('history-recovered', { marker: 'agent_history', data: { ...data, recovered: 'native' } }), created_at: '2026-10-27T12:00:00Z' },
      { ...marker('history-brief', { marker: 'agent_history', data: { ...data, program: 'Claude Code', bytes: null, by: null, recovered: 'brief' } }), created_at: '2026-10-27T12:01:00Z' },
    ] });
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-agent-history]')).toHaveCount(3);
    await settled(page);
    const reads = traffic(page);
    try {
      await page.reload(); await expect(page.locator('[data-agent-history]')).toHaveCount(3); await settled(page);
      const cold = [...reads.requests];
      expect(cold.filter((request) => /\/(?:profiles|users|people)\//.test(request))).toEqual([]);
      await expect(page.locator('[data-agent-history="history-removed"]')).toContainText(/4\s*GiB/);
      await expect(page.locator('[data-agent-history="history-recovered"]')).toContainText(words.agentHistory.history.native);
      await expect(page.locator('[data-agent-history="history-brief"]')).toContainText(words.agentHistory.history.brief);
      await expect(page.locator('[data-agent-history="history-brief"]')).toContainText('–');
      await capture(page, info, `timeline-${suffix}`);
      const idleReads = await idle(page, reads, cold.length);
      const beforeCard = reads.requests.length;
      const actor = page.locator('[data-agent-history="history-removed"]').getByText(ADA.display_name, { exact: true });
      await actor.click();
      await expect(page.getByRole('dialog', { name: ADA.display_name, exact: true })).toBeVisible();
      await settled(page);
      const firstCard = reads.requests.slice(beforeCard);
      await capture(page, info, `person-card-${suffix}`);
      await page.keyboard.press('Escape'); await page.mouse.move(0, 0);
      const beforeWarm = reads.requests.length;
      await actor.click(); await expect(page.getByRole('dialog', { name: ADA.display_name, exact: true })).toBeVisible(); await settled(page);
      await info.attach('requests', { body: JSON.stringify({ cold, idle: idleReads, firstCard, warmCard: reads.requests.slice(beforeWarm) }, null, 2), contentType: 'application/json' });
    } finally { reads.stop(); }
  });

  test(`idle setting, Off and its help ${suffix}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const writes = await historySetting(page);
    const reads = traffic(page);
    try {
      await page.goto('/org/acme.research/settings/policies');
      const row = page.locator(`[data-setting="${KEY}"]`);
      await expect(row.getByRole('spinbutton')).toHaveValue('7'); await settled(page);
      const cold = [...reads.requests];
      await capture(page, info, `setting-default-${suffix}`);
      const idleReads = await idle(page, reads, cold.length);
      const beforeAction = reads.requests.length;
      await row.getByRole('spinbutton').fill('0'); await row.getByRole('spinbutton').press('Control+Enter');
      await expect.poll(() => writes).toEqual([{ value: 0, expected_revision: 0 }]);
      await expect(row.getByText(words.common.off, { exact: true }).first()).toBeVisible();
      await capture(page, info, `setting-off-${suffix}`);
      const beforeHelp = reads.requests.length;
      await row.locator('[data-help-article="settings-and-usage"][data-help-heading="agent-history"]').click();
      await page.getByRole('button', { name: words.helpRelease.openEntry, exact: true }).click();
      await expect(page.getByRole('heading', { name: locale === 'en' ? 'Agent history' : '代理历史', exact: true })).toBeVisible();
      await capture(page, info, `setting-help-${suffix}`);
      expect(reads.requests.slice(beforeHelp), 'The guide is shipped with the application').toEqual([]);
      await info.attach('requests', { body: JSON.stringify({ cold, idle: idleReads, action: reads.requests.slice(beforeAction) }, null, 2), contentType: 'application/json' });
    } finally { reads.stop(); }
  });

  test(`native deletion check labels ${suffix}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await asAdmin(page, true); await language(page, locale);
    await historyBuilds(page);
    const reads = traffic(page);
    try {
      await page.goto('/admin/harnesses'); await settled(page);
      const cold = [...reads.requests];
      const idleReads = await idle(page, reads, cold.length);
      const beforeAction = reads.requests.length;
      await page.getByRole('button', { name: words.admin.harnesses.status.checked, exact: true }).first().click();
      await expect(page.getByText(new RegExp(words.agentHistory.nativeDelete))).toBeVisible();
      await expect(page.getByText(new RegExp(words.agentHistory.safeFallback))).toBeVisible();
      await capture(page, info, `check-results-${suffix}`);
      expect(reads.requests.slice(beforeAction)).toEqual([]);
      await page.keyboard.press('Escape');
      await info.attach('requests', { body: JSON.stringify({ cold, idle: idleReads, action: [] }, null, 2), contentType: 'application/json' });
    } finally { reads.stop(); }
  });
}
