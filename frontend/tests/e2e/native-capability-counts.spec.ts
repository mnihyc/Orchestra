/** EN: Usage → Requests → native quantities → the original turn; no extra request timing.
 * 中文：用量→请求→原生能力用量→原始轮次；不伪造请求耗时。 */
import { expect, test, type Locator } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { nativeTokens } from '../fixtures/native-counts';
import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, emit, scenario, signIn } from './support';
import { seedNativeCounts } from './usage-support';

function fact(row: Locator, label: string) {
  return row.locator('[data-resource-facts] > [data-item]').filter({
    has: row.page().getByText(`${label}:`, { exact: true }),
  });
}

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`late native request keeps its facts beside the already billed aggregate ${language}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await seedNativeCounts(true);
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
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
    await expect(fact(detail, words.usageRequest.attempt).getByRole('definition')).toHaveText('1');
    expect(await table.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`late-native-detail-${language}-${theme}-${width}.png`) });
    await detail.getByRole('link').first().click();
    await expect(page).toHaveURL(/\/thread\/.+\?turn=\d+/);
  });
  test(`native capability quantities click path ${language}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await seedNativeCounts();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
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
    await expect(fact(row, words.capabilityRoutes.names.web_search)).toHaveCount(1);
    await expect(fact(row, words.capabilityRoutes.names.web_search).getByRole('definition')).toHaveText('2');
    await expect(fact(row, words.capabilityRoutes.names.page_fetch).getByRole('definition')).toHaveText('0');
    await expect(fact(row, 'extra_capacity').getByRole('definition')).toHaveText(words.usagePerf.dash);
    await expect(fact(row, words.usageRequest.attempt)).toHaveCount(0);
    await expect(row.getByText(words.usagePerf.outcomes.done, { exact: true })).toHaveCount(0);
    expect(await row.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await info.attach(`native-counts-${language}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await row.getByRole('link').first().click();
    await expect(page).toHaveURL(/\/thread\/.+\?turn=\d+/);
    await page.goBack();
    await expect(page).toHaveURL(/\/usage\?view=requests&range=7d$/);
    await expect(fact(table, words.capabilityRoutes.names.web_search).getByRole('definition')).toHaveText('2');
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
