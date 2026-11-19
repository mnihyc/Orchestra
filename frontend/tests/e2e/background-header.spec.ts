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
    await route.fulfill({ json: { ...current, can_end: canEnd && current.can_end, list_ref: `review-${reads}`, expires_at: new Date(Date.now() + 30_000).toISOString() } satisfies ThreadBackground });
  });
  return {
    get reads() { return reads; },
    readonly() { canEnd = false; },
    async retire() { await deny('ada', ['thread.view']); await publish(current); },
    publish,
  };
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
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
    expect(requests.length, requests.join('\n')).toBeLessThanOrEqual(8);
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
      await expect(value.state === 'live' && value.total !== null ? count() : header.getByRole('button', { name: words.running.title, exact: true })).toBeVisible();
      await openView();
      const result = await page.evaluate(inspectLayout, LAYOUT_INPUT);
      expect(result.faults).toEqual([]);
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
    await expect(dialog().getByRole('button', { name: words.running.end, exact: true })).toHaveCount(0);
    await expect(dialog().getByRole('checkbox')).toHaveCount(0);
    await audit();
    await native.publish(roster({ total: 0, items: [], can_end: false }));
    await expect(header.getByRole('button', { name: words.running.count.replace('{{count}}', '0'), exact: true, includeHidden: true })).toHaveAttribute('aria-expanded', 'true');
    await expect(dialog().getByText('sleep 300', { exact: true })).toBeVisible();
    await page.keyboard.press('Escape'); await expect(dialog()).toHaveCount(0);
    await expect(actions()).toBeFocused();
    await expect(header.getByRole('button', { name: words.running.count.replace('{{count}}', '0'), exact: true })).toHaveCount(0);
    await test.info().attach('complete-api-requests', { body: JSON.stringify({ count: requests.length, requests, reviewReads: native.reads }), contentType: 'application/json' });
  });
}
