/** EN: Usage → Requests → native quantities → the original turn; no extra request timing.
 * 中文：用量→请求→原生能力用量→原始轮次；不伪造请求耗时。 */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { nativeTiming, nativeTokens } from '../fixtures/native-counts';
import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, emit, scenario, signIn } from './support';

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`late native request keeps its facts beside the already billed aggregate ${language}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.route('**/api/v1/usage/events?*', async route => {
      const response = await route.fetch();
      const original = await response.json() as { items: Record<string, unknown>[] };
      const source = original.items.find(item => item.thread !== null && typeof item.turn_ordinal === 'number');
      if (source === undefined) throw new Error('the usage fixture needs an original readable turn');
      const observed = typeof source.timing === 'object' && source.timing !== null ? source.timing : {};
      const aggregate = { ...source, id: '00000000-0000-4000-8000-000000000901', scope: 'turn', origin: 'harness',
        timing: { ...nativeTiming, declared_counters: { web_search: { state: 'told', value: 3 } }, web_searches: { state: 'told', value: 3 } },
        tokens: nativeTokens, request_ref: null, credits: '0.030000' };
      const detail = { ...source, id: '00000000-0000-4000-8000-000000000902', scope: 'request', origin: 'harness',
        timing: { ...observed, declared_counters: { web_search: { state: 'told', value: 1 } }, web_searches: { state: 'told', value: 1 }, outcome: { state: 'told', value: 'done' }, attempt: 1 },
        request_ref: 'late-native-request', credits: '0.000000' };
      await route.fulfill({ response, json: { ...original, items: [aggregate, detail], next_cursor: null } });
    });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    await page.goto('/usage?view=requests&range=7d');
    const table = page.getByRole('table', { name: words.usagePerf.requestsView, exact: true });
    const aggregate = table.getByRole('row').filter({ hasText: words.usageRequest.nativeAggregate });
    await expect(aggregate).toHaveCount(1);
    const detail = table.getByRole('row').filter({ hasText: words.usagePerf.outcomes.done });
    await expect(detail).toHaveCount(1);
    await expect(aggregate.getByText(words.usagePerf.outcomes.done, { exact: true })).toHaveCount(0);
    await expect(detail.getByText(words.usageRequest.nativeAggregate, { exact: true })).toHaveCount(0);
    await expect(detail.getByText(words.usageRequest.attempt, { exact: true })).toBeVisible();
    expect(await table.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`late-native-detail-${language}-${theme}-${width}.png`) });
    await detail.getByRole('link').first().click();
    await expect(page).toHaveURL(/\/thread\/.+\?turn=\d+/);
  });
  test(`native capability quantities click path ${language}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.route('**/api/v1/usage/events?*', async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { items: Record<string, unknown>[] };
      const source = original.items.find((item) => item.thread !== null && typeof item.turn_ordinal === 'number');
      if (source === undefined) throw new Error('the usage fixture needs a readable recorded turn');
      const aggregate = { ...source, scope: 'turn', origin: 'harness', timing: nativeTiming, tokens: nativeTokens, request_ref: null, credits: '0.020000' };
      await route.fulfill({ response, json: { ...original, items: [aggregate], next_cursor: null } });
    });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    await page.goto('/usage?view=requests&range=7d');
    const table = page.getByRole('table', { name: words.usagePerf.requestsView, exact: true });
    const row = table.getByRole('row').filter({ hasText: words.usageRequest.nativeAggregate });
    await expect(row).toHaveCount(1);
    await expect(row.getByText(words.capabilityRoutes.names.web_search, { exact: true })).toHaveCount(1);
    await expect(row.getByText(words.capabilityRoutes.names.page_fetch, { exact: true })).toBeVisible();
    await expect(row.getByText('extra_capacity', { exact: true })).toBeVisible();
    await expect(row.getByText(words.usageRequest.attempt, { exact: true })).toHaveCount(0);
    await expect(row.getByText(words.usagePerf.outcomes.done, { exact: true })).toHaveCount(0);
    expect(await row.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await info.attach(`native-counts-${language}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await row.getByRole('link').first().click();
    await expect(page).toHaveURL(/\/thread\/.+\?turn=\d+/);
    await page.goBack();
    await expect(page).toHaveURL(/\/usage/);
    await expect(page.getByRole('table', { name: words.usagePerf.requestsView, exact: true }).getByText(words.capabilityRoutes.names.web_search, { exact: true })).toBeVisible();
  });
}

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a known native count does not invent model tokens ${language}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    const at = '2026-10-14T10:00:00.000Z';
    const model = { model: 'Search helper', tokens: nativeTokens, tokens_reported: false, counts: { web_search: 2 } };
    await emit(ACME.mainThread, {
      items: [userMessage('native-u', 'Read the report', { author: ADA, turn: 'native-meter', at }), agentMessage('native-a', 'Report ready', { turn: 'native-meter', at, phase: 'final' })],
      inputs: [threadInput('native-input', 'Read the report', { author: ADA, position: '1', state: 'delivered' })],
      turns: [turn('native-meter', { ordinal: 1, state: 'completed', inputId: 'native-input', startedAt: at, completedAt: at, tokens: 100,
        detail: { subagents: null, by_subagent: [], models: [model], context_window: null, context_used: null, compact_at: null, shortenings: [], breakdown: false } })],
      thread: { state: 'idle' },
    });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    await page.goto(`/thread/${ACME.mainThread}`);
    const footer = page.locator('[data-turn-cost-open="native-meter"]');
    await footer.focus();
    await page.keyboard.press('Enter');
    const details = page.locator('[data-turn-cost="native-meter"]');
    await expect(details).toBeVisible();
    const share = details.getByRole('listitem').filter({ hasText: 'Search helper' });
    await expect(share).toHaveCount(1);
    await expect(share).toContainText(words.usagePerf.notMeasured);
    expect(await details.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await info.attach(`native-unknown-tokens-${language}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });
}
