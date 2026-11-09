/** Proven web results, failed calls and unproven attempts retain their real timeline and raw source. */
import { expect, test } from '@playwright/test';
import type { FilePreview } from '../../src/features/files/source';
import type { Item } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, agentMessage, errorItem, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';

const HASH = '9'.repeat(64);
const ORIGINAL = JSON.stringify({ native_result: { type: 'function_call_output', call_id: 'proven-search',
  output: [{ type: 'input_text', text: 'No results' }] }, reported_result: [] });
const ADDRESS = 'https://source.invalid/page';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`web proof failures preserve a completed text turn ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const proof = toolCall('proven-search', { kind: 'web_search', action: { type: 'search' }, query: 'A real empty answer', results: [] },
      { title: 'Search the web', turn: 'web-proof-turn', at: AT });
    if (proof.kind !== 'tool_call') throw new Error('The proven search is a tool');
    const search: Item = { ...proof, payload: { ...proof.payload, raw: { ...proof.payload.raw,
      output: { inline: '', total_len: Buffer.byteLength(ORIGINAL), truncated: true, blob: HASH } } } };
    const opened = toolCall('proven-page', { kind: 'web_search', action: { type: 'open_page', url: ADDRESS }, query: ADDRESS, results: [] },
      { title: 'Open page', turn: 'web-proof-turn', at: AT });
    const failed = toolCall('failed-search', { kind: 'web_search', action: { type: 'search' }, query: 'An HTTP failure', results: [] },
      { title: 'Search the web', turn: 'web-proof-turn', at: AT, status: 'failed', error: true });
    const unproven = toolCall('unproven-page', { kind: 'web_search', action: { type: 'find_in_page', url: ADDRESS, pattern: 'needle' }, query: 'needle', results: [] },
      { title: 'Find in page', turn: 'web-proof-turn', at: AT, status: 'failed', error: true });
    const notice = (id: string, parent: string, knownFailure: boolean): Item => ({
      ...errorItem(id, 'Native web observation', { turn: 'web-proof-turn', at: AT }), kind: 'error', parent_item_id: parent,
      payload: { code: knownFailure ? 'native_web_action_failed' : 'native_web_action_unproven', message: 'Native web observation', retryable: false },
    });
    await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('web-proof-input', 'Inspect the web results', { author: ADA, turn: 'web-proof-turn', inputId: 'web-proof-input', at: AT }),
        search, opened, failed, notice('web-failure', 'failed-search', true), unproven, notice('web-unproven', 'unproven-page', false),
        agentMessage('web-final', 'Web work finished', { turn: 'web-proof-turn', at: AT, phase: 'final' })],
      turns: [turn('web-proof-turn', { ordinal: 1, state: 'completed', inputId: 'web-proof-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('web-proof-input', 'Inspect the web results', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'web-proof-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: HASH, size: Buffer.byteLength(ORIGINAL), media_type: 'text/plain', is_text: true,
      image: null, text: ORIGINAL, width: null, height: null, duration_seconds: null, entry_count: null };
    let fullReads = 0;
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${HASH}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else { fullReads += 1; await route.fulfill({ contentType: 'text/plain', body: ORIGINAL }); }
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    await expect(page.getByText('Web work finished', { exact: true })).toBeVisible();
    const fold = page.locator('[data-fold="web-proof-turn"]');
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(page.locator('[data-item="web-failure"]').getByRole('alert')).toContainText(words.ui.error.nativeWebActionFailed);
    await expect(page.locator('[data-item="web-unproven"]').getByRole('alert')).toContainText(words.ui.error.nativeWebActionUnproven);
    const step = page.locator('[data-step="proven-search"]');
    await step.locator('[data-tool-row] > button[aria-expanded]').click();
    await expect(step.getByText(words.timeline.web.noResults, { exact: true })).toBeVisible();
    const pageStep = page.locator('[data-step="proven-page"]');
    await pageStep.locator('[data-tool-row] > button[aria-expanded]').click();
    await expect(pageStep.getByRole('link')).toHaveAttribute('href', ADDRESS);
    await step.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    const whole = step.getByRole('button', { name: words.timeline.raw.fullOutput, exact: true });
    expect(fullReads).toBe(0);
    await whole.focus();
    await page.keyboard.press('Enter');
    const card = page.locator('[data-file-card]').filter({ hasText: words.timeline.raw.fullOutput });
    await card.getByRole('button', { name: words.fileMedia.open, exact: true }).click();
    const viewer = page.getByRole('dialog', { name: words.timeline.raw.fullOutput, exact: true });
    await expect(viewer.locator('[data-viewer="code"]')).toContainText('native_result');
    await expect(viewer.locator('[data-viewer="code"]')).toContainText('reported_result');
    expect(fullReads).toBeGreaterThan(0);
    await page.keyboard.press('Escape');
    await expect(viewer).toHaveCount(0);
    await expect(whole).toBeFocused();
    await expect(page.locator('[data-thread-header]')).toContainText(words.state.idle);
    await page.reload();
    await expect(page.getByText('Web work finished', { exact: true })).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(page.locator('[data-step="proven-search"]')).toHaveCount(1);
    await expect(page.locator('[data-item="web-unproven"]')).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('native-web-proof', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
