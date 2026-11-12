/** A paused service uses the existing thread recovery line, without keeping the model turn open. */
import { expect, test } from '@playwright/test';
import type { CallRecovery } from '../../src/generated/api-types';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { toolCall } from '../doubles/thread-rows';
import { ACME, emit, scenario, signIn } from './support';
import { chats } from './chat-support';

function pausedRecovery(): CallRecovery {
  return { service: 'web_search', state: 'waiting',
    failure: { code: 'unavailable', message: 'The selected service is unavailable', retryable: true, window: null, said: null, resets_at: null, incomplete: false },
    rule_id: 'pause-web', step: { type: 'wait', until: 'person' }, attempt: 1, due_at: null, detail: null, count: null, window_secs: null };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`paused web service uses existing recovery ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await chats('/locale', { as: 'ada', locale });
    await signIn(page);
    const words = locale === 'en' ? en : zh;
    let recovery: CallRecovery | null = pausedRecovery();
    const actions: string[] = [];
    let inputs = 0;
    page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/inputs')) inputs++; });
    await page.route(`**/api/v1/threads/${ACME.mainThread}/calls/recovery`, async (route) => {
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON() as { action: string };
        actions.push(body.action);
        recovery = null;
      }
      await route.fulfill({ json: { recovery } });
    });
    const paused = { state: 'paused', service: 'web_search', reason: 'wait_for_person', cause: 'unavailable', until: null, needs_person: true };
    await emit(ACME.mainThread, { items: [toolCall('service-paused', {
      kind: 'orch', tool: 'web_search', arguments: { query: 'Exact paused query' }, result: paused,
    }, { title: 'Search the web', name: 'web_search', output: paused })] });
    // EN: Open the thread, read the paused service in its usual recovery line, then Resume service.
    // 中文：打开线程，在原有恢复状态行查看暂停的服务，再点击“恢复服务”。
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-call-recovery="waiting"]')).toContainText(words.capabilityRoutes.names.web_search);
    await expect(page.getByRole('button', { name: words.calls.resumeService, exact: true })).toBeVisible();
    await page.getByRole('button', { name: words.calls.resumeService, exact: true }).click();
    await expect.poll(() => actions.length).toBe(1);
    expect(actions).toEqual(['retry_now']);
    await expect(page.locator('[data-call-recovery="waiting"]')).toHaveCount(0);
    expect(inputs).toBe(0);
    // EN: Reopen after a reset pause; its actual reset time appears without a new per-call control.
    // 中文：重置等待时重新打开线程，显示实际重置时间，不添加单次调用的恢复控件。
    recovery = { service: 'web_search', state: 'scheduled',
      failure: { code: 'rate_limited', message: 'Synthetic rate limit', retryable: true, window: null, said: null, resets_at: null, incomplete: false },
      rule_id: 'pause-web', step: { type: 'wait', until: 'reset' }, attempt: 1, due_at: '2099-11-09T12:00:00Z', detail: null, count: null, window_secs: null };
    await page.reload();
    await expect(page.locator('time[datetime="2099-11-09T12:00:00Z"]')).toBeVisible();
    await expect(page.getByRole('button', { name: words.calls.resumeService, exact: true })).toBeVisible();
    expect(inputs).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`service-pause-${locale}-${theme}-${width}.png`), fullPage: true });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`paused service recovery read stays visible ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await chats('/locale', { as: 'ada', locale });
    await signIn(page);
    const words = locale === 'en' ? en : zh;
    const gate: { release?: () => void } = {};
    const pending = new Promise<void>((resolve) => { gate.release = resolve; });
    let denied = true;
    const actions: string[] = [];
    let inputs = 0;
    page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/inputs')) inputs++; });
    await page.route(`**/api/v1/threads/${ACME.mainThread}/calls/recovery`, async (route) => {
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON() as { action: string };
        actions.push(body.action);
        await route.fulfill({ json: { recovery: null } });
        return;
      }
      await pending;
      if (denied) await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: 'Hidden provider, grant and audience', status: 403, request_id: 'private-service-read' } });
      else await route.fulfill({ json: { recovery: pausedRecovery() } });
    });
    const paused = { state: 'paused', service: 'web_search', reason: 'wait_for_person', cause: 'unavailable', until: null, needs_person: true };
    await emit(ACME.mainThread, { items: [toolCall('service-read-paused', {
      kind: 'orch', tool: 'web_search', arguments: { query: 'Exact paused query' }, result: paused,
    }, { title: 'Search the web', name: 'web_search', output: paused })] });
    try {
      // EN: Open the paused-service thread → checking recovery → read failure → Try again → Resume service.
      // 中文：打开含暂停服务的线程 → 检查恢复状态 → 读取失败 → 重试 → 恢复服务。
      await page.goto(`/thread/${ACME.mainThread}`);
      await expect(page.getByText(words.calls.checkingRecovery, { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: words.calls.resumeService, exact: true })).toHaveCount(0);
      expect(actions).toEqual([]);
      gate.release?.();
      const failure = page.getByRole('alert').filter({ hasText: words.calls.recoveryUnavailable });
      await expect(failure).toBeVisible();
      await expect(failure).not.toContainText('Hidden provider, grant and audience');
      await page.screenshot({ path: info.outputPath(`service-read-failed-${locale}-${theme}-${width}.png`), fullPage: true });
      denied = false;
      await failure.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      await expect(page.locator('[data-call-recovery="waiting"]')).toBeVisible();
      expect(actions).toEqual([]);
      await page.getByRole('button', { name: words.calls.resumeService, exact: true }).click();
      await expect.poll(() => actions.length).toBe(1);
      expect(actions).toEqual(['retry_now']);
      expect(inputs).toBe(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    } finally { gate.release?.(); }
  });
}
