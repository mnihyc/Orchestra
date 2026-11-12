/** Native MCP image children use the same authorized viewer as every other thread image. */
import { expect, test } from '@playwright/test';
import type { FilePreview } from '../../src/features/files/source';
import type { Item } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, errorItem, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';

const HASH = 'c'.repeat(64);
const NAME = 'orchard.png';
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a returned MCP image opens with its call and a failed sibling stays visible ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const parent = toolCall('gallery-call', { kind: 'mcp', server: 'gallery', tool: 'render', arguments: { prompt: 'An orchard' },
      result: { content: [{ type: 'text', text: 'Two views' }, { type: 'image', mimeType: 'image/png', name: NAME }] }, error: null },
    { title: 'gallery: render', turn: 'mcp-turn', at: AT });
    const image: Item = { ...toolCall('returned-picture', { kind: 'image', blob: HASH, media_type: 'image/png', name: NAME, caption: null },
      { title: 'Image', turn: 'mcp-turn', at: AT, durationMs: null }), parent_item_id: 'gallery-call' };
    const failed: Item = { ...errorItem('unretained-picture', 'The returned image could not be retained.', { turn: 'mcp-turn', at: AT }),
      parent_item_id: 'gallery-call', kind: 'error', payload: { code: 'image_result_unavailable', message: 'The returned image could not be retained.', retryable: false } };
    await emit(ACME.mainThread, {
      thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('gallery-input', 'Show the orchard', { author: ADA, turn: 'mcp-turn', inputId: 'gallery-input', at: AT }), parent, image, failed],
      turns: [turn('mcp-turn', { ordinal: 1, state: 'completed', inputId: 'gallery-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('gallery-input', 'Show the orchard', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'mcp-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: HASH, size: Buffer.from(PIXEL, 'base64').length, media_type: 'image/png', is_text: false,
      image: `data:image/png;base64,${PIXEL}`, text: null, width: 1, height: 1, duration_seconds: null, entry_count: null };
    let fullReads = 0;
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${HASH}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else { fullReads += 1; await route.fulfill({ contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') }); }
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
    await expect(page.getByText('Show the orchard', { exact: true })).toBeVisible();
    const fold = page.locator('[data-fold="mcp-turn"]');
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    const call = page.locator('[data-step="gallery-call"]');
    await call.locator('[data-tool-row] > button[aria-expanded]').click();
    await call.getByRole('button', { name: words.ui.json.expand.replace('{{name}}', `${words.timeline.mcp.result}.content.0`), exact: true }).click();
    await expect(call).toContainText('Two views');
    await expect(call).not.toContainText(PIXEL);
    const picture = page.locator('[data-step="returned-picture"]');
    await picture.locator('[data-tool-row] > button[aria-expanded]').click();
    const open = picture.getByRole('button', { name: words.fileMedia.openFile.replace('{{name}}', NAME), exact: true });
    await open.focus();
    await page.keyboard.press('Enter');
    const viewer = page.getByRole('dialog', { name: NAME, exact: true });
    await expect(viewer.locator('[data-viewer="image"] img')).toBeVisible();
    expect(fullReads).toBe(1);
    await page.keyboard.press('Escape');
    await expect(viewer).toHaveCount(0);
    await expect(open).toBeFocused();
    await expect(page.locator('[data-item="unretained-picture"]').getByRole('alert')).toContainText(words.ui.error.imageResultUnavailable);
    await expect(page.locator('[data-thread-header]')).toContainText(words.state.idle);
    await page.reload();
    await expect(page.getByText('Show the orchard', { exact: true })).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(page.locator('[data-step="returned-picture"]')).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`mcp-image-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('mcp-image', { path: screenshot, contentType: 'image/png' });
  });
}
