/** A viewed environment image uses the existing thread blob viewer, including after reload. */
import { expect, test } from '@playwright/test';
import type { FilePreview } from '../../src/features/files/source';
import type { Item } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, errorItem, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';
import { waitForSavedTabId } from './workbench-support';

const IMAGE = 'e'.repeat(64);
const RAW = 'f'.repeat(64);
const NAME = 'environment-view.png';
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const ORIGINAL = JSON.stringify({ type: 'function_call_output', call_id: 'view-call', output: [
  { type: 'input_image', image_url: `data:image/png;base64,${PIXEL}` },
] });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a viewed image and its original result survive the thread click path ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const parent = toolCall('view-call', { kind: 'other', name: 'view_image', arguments: { path: `/workspace/${NAME}` }, result: null },
      { title: 'view_image', turn: 'view-turn', at: AT });
    const picture = toolCall('viewed-picture', { kind: 'image', blob: IMAGE, media_type: 'image/png', name: NAME, caption: null },
      { title: 'Image', turn: 'view-turn', at: AT, durationMs: null });
    if (picture.kind !== 'tool_call') throw new Error('The viewed image is a tool result');
    const image: Item = { ...picture, parent_item_id: 'view-call', payload: { ...picture.payload, raw: {
      ...picture.payload.raw, name: 'image_result', output: { inline: '', total_len: Buffer.byteLength(ORIGINAL), truncated: true, blob: RAW },
    } } };
    const failed: Item = { ...errorItem('missing-view', 'The returned image could not be retained.', { turn: 'view-turn', at: AT }),
      parent_item_id: 'view-call', kind: 'error', payload: { code: 'image_result_unavailable', message: 'The returned image could not be retained.', retryable: false } };
    await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('view-input', 'Inspect the environment picture', { author: ADA, turn: 'view-turn', inputId: 'view-input', at: AT }), parent, image, failed],
      turns: [turn('view-turn', { ordinal: 1, state: 'completed', inputId: 'view-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('view-input', 'Inspect the environment picture', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'view-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: IMAGE, size: Buffer.from(PIXEL, 'base64').length, media_type: 'image/png', is_text: false,
      image: `data:image/png;base64,${PIXEL}`, text: null, width: 1, height: 1, duration_seconds: null, entry_count: null };
    const rawPreview: FilePreview = { ...preview, hash: RAW, size: Buffer.byteLength(ORIGINAL), media_type: 'text/plain', is_text: true,
      image: null, text: ORIGINAL, width: null, height: null };
    let imageReads = 0;
    let rawReads = 0;
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${IMAGE}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else { imageReads += 1; await route.fulfill({ contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') }); }
    });
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${RAW}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: rawPreview });
      else { rawReads += 1; await route.fulfill({ contentType: 'text/plain', body: ORIGINAL }); }
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
    const main = page.getByRole('tablist', { name: words.workbench.areas.main, exact: true });
    const threadTab = main.getByRole('tab', { name: /Main thread/, selected: true });
    await expect(threadTab).toBeVisible();
    const tabId = await waitForSavedTabId(page, threadTab);
    await page.goto('/org/acme');
    if (width === 400) {
      const allTabs = words.workbench.allTabs.replace('{{name}}', words.workbench.areas.main);
      await page.getByRole('button', { name: allTabs, exact: true }).click();
      const target = 'commands:workspace.focus.' + tabId;
      await page.locator(`[role="option"][data-value=${JSON.stringify(target)}]`).click();
    } else await page.locator(`[id=${JSON.stringify(`tab-${tabId}`)}]`).click();
    await expect.poll(() => new URL(page.url()).pathname).toBe(`/thread/${ACME.mainThread}`);
    const fold = page.locator('[data-fold="view-turn"]');
    const step = page.locator('[data-step="viewed-picture"]');
    // The thread is drawn after its address is; the fold, or the step it holds, is what says the turn has been drawn.
    await expect(fold.or(step).first()).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await step.locator('[data-tool-row] > button[aria-expanded]').click();
    const open = step.getByRole('button', { name: words.fileMedia.openFile.replace('{{name}}', NAME), exact: true });
    await open.focus();
    await page.keyboard.press('Enter');
    const viewer = page.getByRole('dialog', { name: NAME, exact: true });
    await expect(viewer.locator('[data-viewer="image"] img')).toBeVisible();
    expect(imageReads).toBeGreaterThan(0);
    expect(rawReads).toBe(0);
    await page.keyboard.press('Escape');
    await expect(viewer).toHaveCount(0);
    await expect(open).toBeFocused();
    await expect(step).not.toContainText(PIXEL);
    await step.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    const whole = step.getByRole('button', { name: words.timeline.raw.fullOutput, exact: true });
    await whole.click();
    const card = page.locator('[data-file-card]').filter({ hasText: words.timeline.raw.fullOutput });
    await card.getByRole('button', { name: words.fileMedia.open, exact: true }).click();
    const raw = page.getByRole('dialog', { name: words.timeline.raw.fullOutput, exact: true });
    await expect(raw.locator('[data-viewer="code"]')).toContainText('function_call_output');
    expect(rawReads).toBeGreaterThan(0);
    await page.keyboard.press('Escape');
    await expect(raw).toHaveCount(0);
    await expect(whole).toBeFocused();
    await expect(page.locator('[data-item="missing-view"]').getByRole('alert')).toContainText(words.ui.error.imageResultUnavailable);
    await expect(page.locator('[data-thread-header]')).toContainText(words.state.idle);
    await page.reload();
    await expect(page.getByText('Inspect the environment picture', { exact: true })).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(step).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('viewed-image-result', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
