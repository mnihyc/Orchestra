/** Confirmed native programs and exact environment consent use existing screens and wire receipts. */
import AxeBuilder from '@axe-core/playwright';
import { expect, type Locator, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { inspectLayout, type DuplicateAllowance } from '../../e2e/layout-rules-dom';
import { LAYOUT_INPUT } from '../../e2e/layout-rules-helpers';
import { executionDisplay } from '../doubles/thread-display';
import { RUNS_ON } from '../doubles/fixtures';
import { ACME, bodySent, replySent, scenario, signIn, arrived } from './support';

import { environmentReferenceAllowance } from './environment-reference-audit';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const KEY = 'exec.version_overrides';
const codex = { kind: 'codex', path: '/usr/local/bin/codex', version: '0.164.0', daemon: true };
const claude = { kind: 'claude-code', path: '/usr/local/bin/claude', version: '2.1.296', daemon: false };
async function control(path: string, data: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) });
  expect(response.ok).toBe(true);
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function object(value: unknown): Record<string, unknown> {
  if (!record(value)) throw new Error('The native API must return its declared object');
  return value;
}

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`machine programs and current tool consent ${width}px ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await control('/resources/machine-programs', { machine_id: RUNS_ON.laptop, harnesses: [claude], discovered_harnesses: [codex], searched_harness_kinds: ['codex', 'claude-code'], delegated: true });
    await signIn(page);
    await control('/first-steps/appearance', { locale, theme: 'system' });
    const words = locale === 'en' ? english : chinese;
    const requests: string[] = [];
    page.on('request', (request) => { const url = new URL(request.url()); if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push(`${request.method()} ${url.pathname}${url.search}`); });
    const snapshot = async (name: string, allowance?: DuplicateAllowance) => {
      await page.mouse.move(0, 0); await page.evaluate(async () => { await document.fonts.ready; });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      const layout = await page.evaluate(inspectLayout, { ...LAYOUT_INPUT, duplicateAllowList: allowance === undefined ? LAYOUT_INPUT.duplicateAllowList : [...LAYOUT_INPUT.duplicateAllowList, allowance] }); expect(layout.faults).toEqual([]);
      const ax = await new AxeBuilder({ page }).analyze();
      expect(ax.violations.filter((row) => row.impact === 'serious' || row.impact === 'critical')).toEqual([]);
      await info.attach(name, { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
    };
    const cold = async (path: string, ready: () => Locator, name: string) => {
      // Retire the setup document before observing the cold target; cookies and current authority stay.
      await page.goto('about:blank'); const start = requests.length;
      await page.goto(path); await expect(ready()).toBeVisible(); await page.waitForLoadState('networkidle');
      await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      const initial = requests.slice(start); expect(initial.length, initial.join('\n')).toBeLessThanOrEqual(8);
      await info.attach(`${name}-initial-requests`, { body: JSON.stringify({ count: initial.length, requests: initial }, null, 2), contentType: 'application/json' });
    };
    const region = () => page.getByRole('region', { name: words.machinePrograms.nativeTitle, exact: true });
    const codexRow = () => region().getByRole('listitem').filter({ has: page.getByText('Codex', { exact: true }) });
    const nativeAction = async (action: 'confirm_harness' | 'remove_harness' | 'reload', run: () => Promise<void>) => {
      const pending = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/machines/${RUNS_ON.laptop}/programs`);
      await run(); const reply = await pending; expect(reply.status()).toBe(200);
      expect(await bodySent(reply.request())).toEqual(action === 'reload' ? { action } : { action, kind: codex.kind, path: codex.path });
    };

    await cold(`/resources/machines/${RUNS_ON.laptop}?tab=programs`, () => codexRow().getByRole('button', { name: words.machinePrograms.nativeUse, exact: true }), 'machine');
    await snapshot('machine-found');
    await nativeAction('confirm_harness', () => codexRow().getByRole('button', { name: words.machinePrograms.nativeUse, exact: true }).click());
    await expect(codexRow().getByText(words.machinePrograms.nativeConfirmed, { exact: true })).toBeVisible();
    await snapshot('machine-confirmed');
    const remove = () => codexRow().getByRole('button', { name: words.machinePrograms.nativeRemove, exact: true });
    await remove().focus(); await page.keyboard.press('Enter'); await expect(page.getByRole('alertdialog')).toBeVisible();
    await snapshot('machine-remove-dialog'); await page.keyboard.press('Escape'); await expect(page.getByRole('alertdialog')).toHaveCount(0); await expect(remove()).toBeFocused();
    await nativeAction('reload', () => page.getByRole('button', { name: words.machinePrograms.nativeRefresh, exact: true }).click());

    const tools = () => page.getByRole('region', { name: words.environmentTools.codex, exact: true });
    const consent = () => tools().getByRole('switch');
    const opening = arrived(page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname === `/api/v1/environments/${RUNS_ON.workspace}/sync`));
    await cold(`/resources/environments/${RUNS_ON.workspace}?tab=tools`, tools, 'environment');
    let currentEnvironment = object(object(await (await opening).json()).environment);
    const environmentTab = page.getByRole('tablist', { name: words.workbench.areas.main, exact: true }).getByRole('tab', { selected: true });
    const environmentTabId = await environmentTab.getAttribute('id'), environmentTabUrl = page.url();
    if (environmentTabId === null) throw new Error('The selected Environment tab must retain its native identity');
    await expect(consent()).not.toBeChecked(); await snapshot('environment-off');
    const write = async (allow: boolean) => {
      const before = object(currentEnvironment.config), decisionsBefore = record(before[KEY]) ? before[KEY] : {};
      const decisionsExpected = allow ? { ...decisionsBefore, codex: { path: codex.path, version: codex.version } }
        : Object.fromEntries(Object.entries(decisionsBefore).filter(([kind]) => kind !== 'codex'));
      const start = requests.length;
      const pending = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname === `/api/v1/environments/${RUNS_ON.workspace}`);
      await consent().click(); const reply = await pending; expect(reply.status()).toBe(200);
      expect(await bodySent(reply.request())).toEqual({ expected_revision: currentEnvironment.revision, config: { ...before, [KEY]: decisionsExpected } });
      currentEnvironment = object(await replySent(reply)); await expect(consent()).toBeChecked({ checked: allow });
      await page.waitForLoadState('networkidle');
      expect(requests.slice(start).filter((request) => request.startsWith('GET '))).toEqual([]);
      return currentEnvironment;
    };
    const saved = await write(true), projected = object(saved.exec).machine_programs;
    if (!Array.isArray(projected) || projected.length !== 1) throw new Error('The confirmed Codex must have one derived executor row');
    const approval = object(object(projected[0]).approval), by = object(approval.by);
    expect(typeof by.display_name).toBe('string');
    await expect(consent()).toHaveAccessibleName(words.environmentTools.anyVersionBy.replace('{{name}}', String(by.display_name)));
    await snapshot('environment-on');
    // Capture the native bootstrap before delivery: the reload can retire its CDP body handle.
    let received!: (value: Record<string, unknown>) => void;
    const reopenedBootstrap = new Promise<Record<string, unknown>>((resolve) => { received = resolve; });
    const bootstrapPath = '**/api/v1/me/bootstrap';
    await page.route(bootstrapPath, async (route) => {
      const response = await route.fetch(); expect(response.status()).toBe(200);
      const startup = object(await response.json());
      await route.fulfill({ response }); received(startup);
    });
    try {
      await page.reload();
      const startup = await reopenedBootstrap;
      expect(object(startup.person).id).toBe(by.id);
      expect(object(startup.admin_mode).as_admin).toBe(false);
    } finally { await page.unroute(bootstrapPath); }
    await expect(consent()).toBeChecked(); await page.waitForLoadState('networkidle');
    await write(false); await expect(consent()).toHaveAccessibleName(words.environmentTools.anyVersion);
    await write(true);

    await control('/resources/machine-programs', { machine_id: RUNS_ON.laptop, harnesses: [{ ...codex, version: 'unknown' }, claude], discovered_harnesses: [], searched_harness_kinds: ['codex', 'claude-code'], delegated: true });
    await expect(tools().getByText(words.machinePrograms.nativeUnknownVersion, { exact: true })).toBeVisible(); await expect(consent()).not.toBeChecked();
    await snapshot('environment-unknown');
    const detail = "Codex on ada-laptop is unknown; this agent runs 0.165.0. An environment manager can allow it on the environment's page.";
    await control('/thread/emit', { thread: ACME.mainThread, rows: { thread: { state: 'blocked', blocked: { reason: 'binding_invalid', detail, sandbox: null }, display: executionDisplay('blocked') } } });
    const openEnvironment = () => page.getByRole('link', { name: words.resourceFacts.openEnvironment, exact: true });
    const threadOpening = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname === `/api/v1/threads/${ACME.mainThread}/opening`);
    await cold(`/thread/${ACME.mainThread}`, openEnvironment, 'thread');
    const line = words.environmentTools.versionMismatch.replace('{{machine}}', 'ada-laptop').replace('{{programVersion}}', words.environmentTools.unknownVersion).replace('{{agentVersion}}', '0.165.0');
    await expect(page.getByText(line, { exact: true })).toBeVisible();
    const mismatch = page.getByRole('alert').filter({ has: page.getByText(line, { exact: true }) });
    await expect(mismatch.getByRole('button', { name: words.change.banner.switchProfile, exact: true })).toBeVisible();
    await expect(openEnvironment()).toHaveAttribute('href', `/resources/environments/${RUNS_ON.workspace}?tab=tools`);
    let allowance: DuplicateAllowance | undefined;
    const status = page.locator('[data-status-place="bar"] [data-status-item="environment"] > button');
    if (await status.isVisible()) {
      const nativeOpening = await threadOpening; expect(nativeOpening.status()).toBe(200);
      const nativeThread = object(object(await replySent(nativeOpening)).sync).thread;
      const current = object(object(nativeThread).environment);
      if (typeof current.id !== 'string' || typeof current.name !== 'string' || current.id !== currentEnvironment.id) throw new Error('The current status and retained page must have one native Environment target');
      const tab = page.locator(`[role="tab"][id=${JSON.stringify(environmentTabId)}] > [data-view-tab-title]`);
      const label = await status.getAttribute('aria-label'), statusText = await status.locator(':scope > span.truncate:not([data-resource-hint])').innerText();
      if (label === null) throw new Error('The status reference must retain its qualified accessible name');
      await status.focus(); await page.keyboard.press('Enter');
      const card = page.getByRole('dialog').filter({ has: page.locator('[data-resource-card="environment"]') });
      const link = card.getByRole('link', { name: words.resourceFacts.openEnvironment, exact: true });
      await expect(link).toBeVisible(); const href = await link.getAttribute('href');
      if (href === null) throw new Error('The current status card must expose its actual Environment link');
      allowance = environmentReferenceAllowance({ origin: new URL(page.url()).origin, environment: { id: current.id, name: current.name },
        tab: { id: environmentTabId, url: environmentTabUrl, text: await tab.innerText() }, status: { url: href, label, text: statusText } });
      expect(allowance).toBeDefined();
      await page.keyboard.press('Escape'); await expect(card).toHaveCount(0); await page.mouse.move(0, 0);
      await expect(page.locator('[data-resource-card="environment"]:visible')).toHaveCount(0);
      await info.attach('thread-reference-identity', { body: JSON.stringify({ environment: current, tabId: environmentTabId, tabUrl: environmentTabUrl, statusUrl: href, allowance }, null, 2), contentType: 'application/json' });
    }
    await snapshot('thread-version-blocked', allowance);
    await openEnvironment().click(); await expect(tools()).toBeVisible(); await expect(consent()).not.toBeChecked();
    await cold(`/resources/machines/${RUNS_ON.laptop}?tab=programs`, remove, 'machine-remove');
    await remove().click(); await expect(page.getByRole('alertdialog')).toBeVisible();
    await nativeAction('remove_harness', () => page.getByRole('alertdialog').getByRole('button', { name: words.machinePrograms.nativeRemove, exact: true }).click());
    await expect(page.getByRole('alertdialog')).toHaveCount(0);
    await expect(codexRow().getByRole('button', { name: words.machinePrograms.nativeUse, exact: true })).toBeFocused();
    await cold('/resources/environments', () => page.getByRole('combobox', { name: words.library.show, exact: true }), 'environment-list');
    await expect(page.getByRole('combobox', { name: words.library.show, exact: true })).toHaveText(words.library.views.usable);
    await snapshot('environment-list-usable');
    expect(requests.some((request) => request.startsWith(`POST /api/v1/threads/${ACME.mainThread}/inputs`))).toBe(false);
    await info.attach('all-api-requests', { body: JSON.stringify({ count: requests.length, requests }, null, 2), contentType: 'application/json' });
  });
}
