/** The compatibility observations use the existing timeline and the native generic approval card. */
import { expect, test } from '@playwright/test';
import { errorItem, marker } from '../doubles/thread-rows.ts';
import { ACME, DOUBLE, bodySent, emit, playScene, scenario, signIn } from './support.ts';

const words = {
  en: { event: 'Unrecognized Claude Code event future_update', stream: 'Unrecognized Claude Code event stream_event/future_update', method: 'Unrecognized Claude Code event control_request/future_method', allow: 'Allow once', deny: 'Deny' },
  'zh-CN': { event: '无法识别的 Claude Code 事件 future_update', stream: '无法识别的 Claude Code 事件 stream_event/future_update', method: '无法识别的 Claude Code 事件 control_request/future_method', allow: '允许一次', deny: '拒绝' },
} as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`Claude protocol additions remain visible and decidable (${locale}, ${theme})`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme }), signal: AbortSignal.timeout(10_000),
    });
    expect(appearance.ok).toBe(true);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    const base = { program: 'Claude Code', previous: { state: 'unsupported' as const }, expected: { state: 'unsupported' as const },
      changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: '2026-10-08T14:00:01Z', value: { count: 1 } };
    await emit(ACME.mainThread, { items: [
      marker('claude-future-event', { marker: 'harness_changed', data: { ...base, change: 'unrecognized_event', subject: 'future_update' } }),
      marker('claude-future-method', { marker: 'harness_changed', data: { ...base, change: 'unrecognized_event', subject: 'control_request/future_method' } }),
      marker('claude-future-stream', { marker: 'harness_changed', data: { ...base, change: 'unrecognized_event', subject: 'stream_event/future_update' } }),
      errorItem('claude-malformed-line', 'the harness wrote a line that is not a JSON object: {broken', { code: 'harness_error' }),
    ] });
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(page.getByText(words[locale].event, { exact: true })).toBeVisible();
      await expect(page.getByText(words[locale].method, { exact: true })).toBeVisible();
      await expect(page.getByText(words[locale].stream, { exact: true })).toBeVisible();
      await expect(page.getByText('the harness wrote a line that is not a JSON object: {broken', { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`claude-tolerance-${width}.png`) });
    }
    await playScene(ACME.mainThread, 'everything');
    await page.goto('/approvals');
    const generic = page.getByRole('region', { name: 'Something else the harness asks' });
    await expect(generic.getByText('Open a connection to the build cache', { exact: true })).toBeVisible();
    await expect(generic.getByRole('button', { name: new RegExp(`^${words[locale].allow} ·`) })).toBeVisible();
    const refused = page.waitForRequest(request => request.method() === 'POST' && request.url().includes('/approvals/') && request.url().endsWith('/decision'));
    await generic.getByRole('button', { name: new RegExp(`^${words[locale].deny} ·`) }).click();
    expect(await bodySent(await refused)).toMatchObject({ option_id: 'deny' });
    await expect(generic.getByRole('button', { name: new RegExp(`^${words[locale].allow} ·`) })).toHaveCount(0);
  });
}
