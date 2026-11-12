/** Codex compatibility observations remain readable and its unknown approval can only be cancelled. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { approvalItem, errorItem, harnessApproval, marker } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, DOUBLE, addApproval, bodySent, emit, scenario, signIn } from './support.ts';

const title = 'Unrecognized Codex approval item/futureTool/requestApproval';
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`Codex compatibility facts and cancellation survive the thread click path (${locale}, ${theme})`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }),
      signal: AbortSignal.timeout(10_000),
    });
    expect(appearance.ok).toBe(true);
    const words = locale === 'en' ? english : chinese;
    const unknown = locale === 'en' ? 'Unrecognized Codex event future/update' : '无法识别的 Codex 事件 future/update';
    const counted = `${unknown} · 2`;
    const approval = 'codex-unknown-approval';
    await addApproval(harnessApproval(approval, { threadId: ACME.mainThread, itemId: 'codex-unknown-request', title, requester: ADA,
      request: { request_kind: 'other', options: [{ id: 'cancel_unrecognized_approval', label: 'Cancel request', style: 'deny', scope: null, rule: null }],
        detail: { summary: title, raw: { method: 'item/futureTool/requestApproval', kind: 'futurePermission' } } } }));
    const base = { program: 'Codex', change: 'unrecognized_event', subject: 'future/update', previous: { state: 'unsupported' as const },
      expected: { state: 'unsupported' as const }, changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: '2026-11-08T07:00:00Z' };
    await emit(ACME.mainThread, { items: [
      marker('codex-unknown-idle', { marker: 'harness_changed', data: { ...base, value: { count: 1 } } }),
      marker('codex-managed-mismatch', { marker: 'harness_changed', data: { ...base, change: 'managed_settings_readback', subject: null, diverged: true, value: { status: 'mismatch', pins: ['features.memories'] } } }),
      marker('codex-unknown-counted', { marker: 'harness_changed', data: { ...base, value: { count: 2 } } }),
      errorItem('codex-malformed-frame', 'Codex emitted a malformed protocol frame: {broken fixture', { code: 'harness_error' }),
      approvalItem('codex-unknown-request', approval),
    ] });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(page.getByText(unknown, { exact: true })).toBeVisible();
      await expect(page.getByText(counted, { exact: true })).toBeVisible();
      await expect(page.getByText(locale === 'en' ? 'Codex reported different managed launch settings.' : 'Codex 报告的受管启动设置与要求不同。', { exact: true })).toBeVisible();
      await expect(page.getByText('Codex emitted a malformed protocol frame: {broken fixture', { exact: true })).toBeVisible();
      await expect(page.getByText('private payload', { exact: false })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`codex-tolerance-${width}.png`) });
    }
    await page.goto('/approvals');
    const card = page.getByRole('region', { name: title });
    await expect(card.getByRole('button', { name: words.approvals.allowOnce, exact: true })).toHaveCount(0);
    await expect(card.getByRole('button', { name: words.approvals.allowSession, exact: true })).toHaveCount(0);
    const sent = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === `/api/v1/approvals/${approval}/decision`);
    await card.getByRole('button', { name: new RegExp(`^${words.approvals.deny}`) }).click();
    expect(await bodySent(await sent)).toMatchObject({ option_id: 'cancel_unrecognized_approval' });
  });
}
