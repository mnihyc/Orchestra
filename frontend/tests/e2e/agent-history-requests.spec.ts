/** Managed-history screens add no API requests to the same production frame without the feature. */
import { expect, test, type Browser, type Page, type Request } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import { marker, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, configureThread, emit, scenario, signIn } from './support.ts';
import { asAdmin } from './admin-mode-support.ts';
import { HISTORY_KEY, historyBuilds, historySetting, programExitProvider } from './agent-history-support.ts';

const BASELINE = process.env.ORCH_HISTORY_BASELINE_URL;
const IDLE_MS = 60_000;
type Surface = 'timeline' | 'setting' | 'check' | 'provider';
type Phase = 'cold' | 'idle' | 'action' | 'warm';
type Ledger = Record<Phase, string[]>;

async function settled(page: Page) {
  await page.waitForLoadState('networkidle');
  expect(await page.locator('script[src]').evaluateAll((scripts) => scripts.some((script) => script.getAttribute('src')?.startsWith('/src/') === true))).toBe(false);
  await expect(page.locator('body')).not.toContainText('Unexpected Application Error');
}

async function observe(browser: Browser, address: string, width: number, surface: Surface, feature: boolean) {
  const context = await browser.newContext({ baseURL: address, viewport: { width, height: 1000 }, colorScheme: 'light', locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  const requests: string[] = [];
  const listen = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push(`${request.method()} ${url.pathname}${url.search}`);
  };
  try {
    await scenario();
    if (surface === 'timeline') {
      await configureThread(ACME.mainThread, { reset: true });
      const data = { segment_id: 'kept-segment', program: 'Codex', bytes: 4096, removed_at: '2026-10-20T12:00:00Z', by: ADA, recovered: null } as const;
      await emit(ACME.mainThread, { thread: { state: 'idle' }, items: [
        userMessage('existing-person', 'A retained message', { author: ADA }),
        ...(feature ? [marker('managed-history', { marker: 'agent_history', data })] : []),
      ] });
    }
    await signIn(page);
    // The first addressed page initializes the retained layout; both builds must enter the same page first.
    await expect(page.getByRole('tab', { name: 'Acme', exact: true })).toBeVisible();
    if (surface === 'check') await asAdmin(page, true);
    const writes = surface === 'setting' ? await historySetting(page) : [];
    if (surface === 'check') await historyBuilds(page);
    const provider = surface === 'provider' ? await programExitProvider() : undefined;
    const path = surface === 'timeline' ? `/thread/${ACME.mainThread}` : surface === 'setting' ? '/org/acme.research/settings/policies' : surface === 'provider' && provider !== undefined ? provider.path : '/admin/harnesses';
    const ready = async () => {
      if (surface === 'timeline') await expect(page.locator('[data-item="existing-person"]')).toBeVisible();
      else if (surface === 'setting') await expect(page.locator(`[data-setting="${HISTORY_KEY}"]`).getByRole('spinbutton')).toHaveValue('7');
      else if (surface === 'provider') await expect(page.getByRole('tab', { name: english.providers.tabs.settings, exact: true })).toHaveAttribute('aria-selected', 'true');
      else await expect(page.getByRole('button', { name: english.admin.harnesses.status.checked, exact: true }).first()).toBeVisible();
      await settled(page);
    };
    await page.goto(path); await ready();
    page.on('request', listen);
    await page.reload(); await ready();
    const cold = [...requests];
    const frame = await page.locator('[role="tablist"]').evaluateAll((elements) => elements.filter((element) => element.getClientRects().length > 0)
      .map((element) => ({ name: element.getAttribute('aria-label'), tabs: Array.from(element.querySelectorAll('[role="tab"]')).map((tab) => tab.textContent.trim()) })));
    await page.clock.install(); await page.clock.fastForward(IDLE_MS); await settled(page);
    const idle = requests.slice(cold.length);
    const beforeAction = requests.length;
    if (surface === 'timeline') {
      const person = feature ? page.locator('[data-agent-history="managed-history"]') : page.locator('[data-item="existing-person"]');
      await person.getByText(ADA.display_name, { exact: true }).click();
      await expect(page.getByRole('dialog', { name: ADA.display_name, exact: true })).toBeVisible();
    } else if (surface === 'setting') {
      const field = page.locator(`[data-setting="${HISTORY_KEY}"]`).getByRole('spinbutton');
      await field.fill('0'); await field.press('Control+Enter');
      await expect.poll(() => writes).toEqual([{ value: 0, expected_revision: 0 }]);
    } else if (surface === 'check') {
      await page.getByRole('button', { name: english.admin.harnesses.status.checked, exact: true }).first().click();
      await expect(page.getByRole('dialog', { name: english.admin.harnesses.checkResults, exact: true })).toBeVisible();
    } else {
      await page.getByRole('tab', { name: english.providers.tabs.overview, exact: true }).click();
      await page.getByRole('tab', { name: english.providers.tabs.settings, exact: true }).click();
    }
    await settled(page);
    const action = requests.slice(beforeAction);
    const beforeWarm = requests.length;
    if (surface === 'timeline') {
      await page.keyboard.press('Escape'); await page.mouse.move(0, 0);
      const person = feature ? page.locator('[data-agent-history="managed-history"]') : page.locator('[data-item="existing-person"]');
      await person.getByText(ADA.display_name, { exact: true }).click();
      await expect(page.getByRole('dialog', { name: ADA.display_name, exact: true })).toBeVisible();
      await settled(page);
    } else if (surface === 'provider') {
      await page.getByRole('tab', { name: english.providers.tabs.overview, exact: true }).click();
      await page.getByRole('tab', { name: english.providers.tabs.settings, exact: true }).click();
      await settled(page);
    }
    const ledger: Ledger = { cold, idle, action, warm: requests.slice(beforeWarm) };
    const scripts = await page.locator('script[src]').evaluateAll((elements) => elements.map((element) => element.getAttribute('src')));
    return { ledger, scripts, frame };
  } finally {
    page.off('request', listen);
    await context.close();
  }
}

function additional(current: string[], baseline: string[]) {
  const available = new Map<string, number>();
  for (const request of baseline) available.set(request, (available.get(request) ?? 0) + 1);
  return current.filter((request) => {
    const remaining = available.get(request) ?? 0;
    if (remaining === 0) return true;
    available.set(request, remaining - 1);
    return false;
  });
}

// HTTP counts are independent of theme and catalogue; the separate visual matrix covers both editions and themes.
for (const width of [1440, 400]) for (const surface of ['timeline', 'setting', 'check', 'provider'] as const) {
  test(`managed history adds no frame requests for ${surface} at ${width}px`, async ({ browser, baseURL }, info) => {
    test.skip(BASELINE === undefined, 'Set ORCH_HISTORY_BASELINE_URL to the production build before managed history');
    if (BASELINE === undefined || baseURL === undefined) throw new Error('Both production origins are required');
    const before = await observe(browser, BASELINE, width, surface, false);
    const current = await observe(browser, baseURL, width, surface, true);
    const difference = Object.fromEntries((['cold', 'idle', 'action', 'warm'] as const).map((phase) => [phase, {
      before: before.ledger[phase].length,
      current: current.ledger[phase].length,
      delta: current.ledger[phase].length - before.ledger[phase].length,
      added: additional(current.ledger[phase], before.ledger[phase]),
    }] as const));
    await info.attach('frame-request-difference', { body: JSON.stringify({ before, current, difference }, null, 2), contentType: 'application/json' });
    expect(current.frame, 'Both request ledgers must measure the same visible frame').toEqual(before.frame);
    for (const [phase, result] of Object.entries(difference)) expect(result.added, `${phase}: ${JSON.stringify(result)}`).toEqual([]);
  });
}
