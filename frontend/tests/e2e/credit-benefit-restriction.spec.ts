/** The confirmed provider credit refusal remains specific in the thread and its provider request log. */
import { expect, test } from '@playwright/test';
import type { Item } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { timingFixture } from '../doubles/performance';
import { requestRow } from '../doubles/request-row';
import { AT, errorItem, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';

const NATIVE = 'This organization cannot be used for Codex usage because it is enrolled in the monthly API Credits benefit with ChatGPT subscriptions. To enable Codex usage, an Organization Owner must cancel the benefit on https://platform.openai.com/settings/organization/billing/overview.';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`credit benefit owner fix survives thread and provider navigation ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const failed = turn('credit-turn', { ordinal: 1, state: 'failed', inputId: 'credit-input', startedAt: AT, completedAt: AT });
    if (failed.error === null) throw new Error('The failed turn must carry its ordinary error facts');
    failed.error = { ...failed.error, code: 'model_unavailable', reason: 'credit_program', message: 'This model is unavailable.', said: NATIVE, retryable: false, status: 400 };
    const payload = { code: 'model_unavailable', message: 'This model is unavailable.', retryable: false, details: { reason: 'credit_program' } };
    const notice: Item = { ...errorItem('credit-notice', payload.message, { turn: failed.id, at: AT }), kind: 'error', payload };
    await emit(ACME.mainThread, {
      thread: { state: 'idle', current_turn_id: null }, turns: [failed],
      inputs: [{ ...threadInput('credit-input', 'Use this endpoint', { author: ADA, position: '1', state: 'delivered' }), turn_id: failed.id, delivered_at: AT }],
      items: [userMessage('credit-prompt', 'Use this endpoint', { author: ADA, turn: failed.id, inputId: 'credit-input', at: AT }), notice],
    });
    const base = requestRow();
    const record = requestRow({ at: new Date().toISOString(), provider: { ...base.provider, id: RUNS_ON.openAiKey },
      thread: { id: ACME.mainThread, kind: 'thread', name: 'Main thread', redacted: false }, turn_id: failed.id, turn_ordinal: 1,
      timing: timingFixture({ outcome: { state: 'told', value: 'model_unavailable' }, error_reason: 'credit_program', error_message: payload.message, upstream_status: 400, vendor_words: NATIVE }) });
    await page.route((url) => url.pathname === '/api/v1/usage/events', (route) => route.fulfill({ json: { items: [record], next_cursor: null } }));
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${ACME.mainThread}`);
    const problem = page.getByRole('alert').filter({ hasText: words.ui.error.creditProgramRestricted });
    await expect(problem).toHaveCount(1);
    const original = page.locator('summary').filter({ hasText: words.timeline.failure.said });
    await original.click();
    await expect(page.getByText(NATIVE, { exact: true })).toBeVisible();
    await original.click();
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=overview`);
    await page.getByRole('tab', { name: words.providers.tabs.usage, exact: true }).click();
    await page.getByRole('radiogroup', { name: words.usagePerf.views, exact: true }).getByRole('radio', { name: words.usagePerf.viewNames.requests, exact: true }).click();
    const requests = page.getByRole('table', { name: words.usagePerf.requestsView, exact: true });
    await expect(requests).toContainText(words.usageRequest.reasons.credit_program);
    await expect(requests).toContainText('400');
    await expect(requests).toContainText(NATIVE);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('provider-credit-benefit-refusal', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
    await requests.getByRole('link', { name: 'Main thread', exact: true }).click();
    await expect(problem).toHaveCount(1);
    // EN: Read the thread's owner fix, open its original words, then inspect the provider's request and follow its turn.
    // 中文：查看线程中的所有者处理办法和原始说明，再查看提供方的请求记录，并返回对应轮次。
  });
}
