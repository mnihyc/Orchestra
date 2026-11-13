/** The actual header presents verified native work through one existing reviewed command surface. */
import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, test } from '@playwright/test';
import type { BackgroundCommand, ThreadBackground } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { executionDisplay } from '../doubles/thread-display';
import { inspectLayout } from '../../e2e/layout-rules-dom';
import { LAYOUT_INPUT } from '../../e2e/layout-rules-helpers';
import { ACME, deny, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const BACKGROUND = `/api/v1/threads/${ACME.mainThread}/background`;
const commands: BackgroundCommand[] = ['one', 'two'].map((id) => ({ id: `native-${id}`, segment_id: '00000000-0000-4000-8001-000000000001',
  turn_id: '00000000-0000-4000-8001-000000000002', item_id: null, command: `sleep ${id === 'one' ? 300 : 600}`,
  command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment', since: null, cpu_percent: null, memory_bytes: null }));
const roster = (more: Partial<ThreadBackground> = {}): ThreadBackground => ({ supported: true, state: 'live', total: 2, items: commands,
  as_of: null, next_cursor: null, can_end: true, list_ref: null, expires_at: null, reason: null, ...more });

async function control(path: string, data: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
}

/** A synthetic advertised native source exercises the existing wire contract, not any vendor SDK capability claim. */
async function nativeSource(page: Page) {
  let current = roster(), canEnd = true, reads = 0;
  const publish = async (value: ThreadBackground) => {
    current = value;
    await control('/thread/emit', { thread: ACME.mainThread, rows: { background: value } });
  };
  await publish(current);
  await page.route(`**${BACKGROUND}`, async (route) => {
    reads += 1;
    await route.fulfill({ json: { ...current, can_end: canEnd && current.can_end, end_reason: canEnd ? current.end_reason ?? null : 'interact_required', list_ref: `review-${reads}`, expires_at: new Date(Date.now() + 30_000).toISOString() } satisfies ThreadBackground });
  });
  return {
    get reads() { return reads; },
    readonly() { canEnd = false; },
    async retire() { await deny('ada', ['thread.view']); await publish(current); },
    publish,
  };
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`native End explains its model boundary before confirmation at ${width}px ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await control('/thread/emit', { thread: ACME.mainThread, rows: { thread: { state: 'idle', display: executionDisplay('idle') } } });
    await nativeSource(page);
    await signIn(page);
    await control('/first-steps/appearance', { locale, theme: 'system' });
    const words = locale === 'en' ? english : chinese;
    const writes: string[] = [];
    page.on('request', (entry) => {
      const path = new URL(entry.url()).pathname;
      if (entry.method() === 'POST' && (path === `${BACKGROUND}/end` || path === `/api/v1/threads/${ACME.mainThread}/inputs`)) writes.push(path);
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await page.locator('[data-thread-header]').getByRole('button', { name: words.running.count.replace('{{count}}', '2'), exact: true }).click();
    const work = page.getByRole('dialog', { name: words.running.title, exact: true });
    await expect(work.getByText('sleep 300', { exact: true })).toBeVisible();
    await work.getByRole('button', { name: words.running.end, exact: true }).first().click();
    const confirmation = page.getByRole('alertdialog', { name: words.running.confirmTitle, exact: true });
    const detail = (new Intl.PluralRules(locale).select(1) === 'one'
      ? words.running.confirmDetail_one : words.running.confirmDetail_other).replace('{{count}}', '1');
    await expect(confirmation.getByText(detail, { exact: true })).toBeVisible();
    await expect(confirmation).toContainText(locale === 'en'
      ? 'Orch sends no model prompt; the harness may tell the model the work ended.'
      : 'Orch 不会向模型发送提示；运行程序仍可能通知模型任务已结束。');
    expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('native-end-notice.png'), fullPage: true, animations: 'disabled' });
    await page.keyboard.press('Escape');
    await expect(confirmation).toHaveCount(0);
    expect(writes).toEqual([]);
  });

  test(`native background work stays visible beside Idle at ${width}px ${locale}/${theme}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await control('/thread/emit', { thread: ACME.mainThread, rows: { thread: { state: 'idle', display: executionDisplay('idle') } } });
    const native = await nativeSource(page);
    await signIn(page);
    await control('/first-steps/appearance', { locale, theme: 'system' });
    const words = locale === 'en' ? english : chinese;
    const requests: string[] = [];
    page.on('request', (entry) => { const path = new URL(entry.url()).pathname; if (path.startsWith('/api/v1/') && path !== '/api/v1/ws') requests.push(`${entry.method()} ${path}`); });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const header = page.locator('[data-thread-header]');
    const countLabel = words.running.count.replace('{{count}}', '2');
    const count = () => header.getByRole('button', { name: countLabel, exact: true });
    const actions = () => header.getByRole('button', { name: words.actions.actions.named.replace('{{name}}', 'Main thread'), exact: true });
    await expect(header.getByText(words.state.idle, { exact: true })).toBeVisible();
    await expect(count()).toBeVisible();
    await expect(count()).toHaveAttribute('aria-haspopup', 'dialog');
    await expect(count()).toHaveAttribute('aria-expanded', 'false');
    await page.waitForLoadState('networkidle');
    expect(native.reads).toBe(0);
    // The header uses the opening's roster; other workbench sections own their reads.
    const opening = `GET /api/v1/threads/${ACME.mainThread}/opening`;
    expect(requests.filter((request) => request === opening)).toEqual([opening]);
    expect(requests.filter((request) => request.includes(BACKGROUND))).toEqual([]);
    await test.info().attach('initial-api-requests', { body: JSON.stringify({ count: requests.length, requests }), contentType: 'application/json' });

    const audit = async () => {
      const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
      expect(result.violations.filter((row) => row.impact === 'serious' || row.impact === 'critical').map((row) => row.id)).toEqual([]);
    };
    const image = test.info().outputPath(`background-header-${width}-${locale}-${theme}.png`);
    await page.screenshot({ path: image, fullPage: true, animations: 'disabled' });
    await test.info().attach('background-header', { path: image, contentType: 'image/png' });
    await audit();

    const openView = async () => {
      await actions().click();
      await page.getByRole('menuitem', { name: words.thread.header.view, exact: true }).focus();
      await page.keyboard.press('ArrowRight');
      await expect(page.getByRole('menuitem', { name: words.running.title, exact: true })).toBeVisible();
    };
    for (const value of [roster(), roster({ total: null, items: [], can_end: false }), roster({ state: 'offline', total: null, items: [], can_end: false }),
      roster({ supported: false, state: 'unavailable', total: null, items: [], can_end: false })]) {
      await native.publish(value);
      const control = value.state === 'live' && value.total !== null ? count() : header.getByRole('button', { name: words.running.title, exact: true });
      await expect(control).toBeVisible();
      await expect(control).toHaveText(value.state === 'live' && value.total !== null ? countLabel : words.running.title);
      await openView();
      const result = await page.evaluate(inspectLayout, LAYOUT_INPUT);
      expect(result.faults).toEqual([]);
      if (value.state === 'live' && value.total === null) await test.info().attach('background-unknown-view-menu', {
        body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png',
      });
      await audit();
      await page.keyboard.press('Escape');
      if (await page.getByRole('menu').count() > 0) await page.keyboard.press('Escape');
      await expect(page.getByRole('menu')).toHaveCount(0);
    }
    await native.publish(roster()); await expect(count()).toBeVisible();

    const dialog = () => page.getByRole('dialog', { name: words.running.title, exact: true });
    await count().focus(); await page.keyboard.press('Enter');
    await expect(dialog()).toHaveCount(1);
    await expect(dialog().getByText('sleep 300', { exact: true })).toBeVisible();
    await expect(header.getByRole('button', { name: countLabel, exact: true, includeHidden: true })).toHaveAttribute('aria-expanded', 'true');
    await audit();
    await page.keyboard.press('Escape'); await expect(dialog()).toHaveCount(0); await expect(count()).toBeFocused();
    await openView(); await page.getByRole('menuitem', { name: words.running.title, exact: true }).click();
    await expect(dialog()).toHaveCount(1);
    await page.keyboard.press('Escape'); await expect(dialog()).toHaveCount(0); await expect(actions()).toBeFocused();

    await page.keyboard.press('Control+k');
    const finder = page.getByRole('dialog', { name: words.finder.title, exact: true });
    await expect(finder).toBeVisible();
    await finder.getByRole('combobox').fill(`>${words.running.title}`);
    await finder.locator('[role="option"][data-value="commands:thread.running"]').click();
    await expect(dialog()).toHaveCount(1);
    await page.keyboard.press('Escape'); await expect(dialog()).toHaveCount(0); await expect(actions()).toBeFocused();

    await count().click(); await expect(dialog()).toHaveCount(1);
    await page.keyboard.press('Control+k'); await expect(finder).toBeVisible();
    await finder.getByRole('combobox').fill('>');
    await native.retire();
    await expect(page.getByRole('dialog', { name: words.running.title, exact: true, includeHidden: true })).toHaveCount(0);
    await expect(finder.getByRole('combobox')).toBeFocused();
    await page.keyboard.press('Escape'); await expect(finder).toHaveCount(0);

    await deny('ada', ['thread.interact']); native.readonly();
    await page.reload(); await expect(count()).toBeVisible(); await page.waitForLoadState('networkidle');
    await count().click(); await expect(dialog()).toHaveCount(1);
    await expect(dialog().getByText('sleep 300', { exact: true })).toBeVisible();
    const endControls = dialog().getByRole('button', { name: words.running.end, exact: true });
    await expect(endControls).toHaveCount(commands.length);
    for (const end of await endControls.all()) {
      await expect(end).toHaveAttribute('aria-disabled', 'true');
      await expect(end).toHaveAttribute('aria-description', words.running.endReasons.interact_required);
    }
    await endControls.first().focus();
    await expect(page.getByRole('tooltip')).toContainText(words.running.endReasons.interact_required);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('alertdialog')).toHaveCount(0);
    const unavailable = test.info().outputPath(`background-end-unavailable-${width}-${locale}-${theme}.png`);
    await page.screenshot({ path: unavailable, fullPage: true, animations: 'disabled' });
    await test.info().attach('background-end-unavailable', { path: unavailable, contentType: 'image/png' });
    await expect(dialog().getByRole('checkbox')).toHaveCount(0);
    await audit();
    await native.publish(roster({ total: 0, items: [], can_end: false }));
    await expect(header.getByRole('button', { name: words.running.count.replace('{{count}}', '0'), exact: true, includeHidden: true })).toHaveAttribute('aria-expanded', 'true');
    await expect(dialog().getByText('sleep 300', { exact: true })).toBeVisible();
    // Focus is still on the first End, whose reason is open as a tooltip: Escape closes that inner layer first.
    if (await page.getByRole('tooltip').count() > 0) { await page.keyboard.press('Escape'); await expect(page.getByRole('tooltip')).toHaveCount(0); }
    await page.keyboard.press('Escape'); await expect(dialog()).toHaveCount(0);
    await expect(actions()).toBeFocused();
    await expect(header.getByRole('button', { name: words.running.count.replace('{{count}}', '0'), exact: true })).toHaveCount(0);
    await test.info().attach('complete-api-requests', { body: JSON.stringify({ count: requests.length, requests, reviewReads: native.reads }), contentType: 'application/json' });
  });
}
