/** A native image and its allowance notice use the real timeline, help and authorized file viewer. */
import { expect, test } from '@playwright/test';
import type { FilePreview } from '../../src/features/files/source';
import type { Item, ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, errorItem, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn, supplyDefaults } from './support';

const KEY = 'agents.image_generation';
const RESET = '2026-10-12T12:35:52Z';
const HASH = 'b'.repeat(64);
const NAME = 'generated-image.png';
const CAPTION = 'An orchard in morning light.';
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  const words = locale === 'en' ? english : chinese;
  const suffix = `${locale}/${theme}/${width}`;

  test(`an image allowance keeps the successful text turn and the image opens ${suffix}`, async ({ page }, info) => {
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const payload = { code: 'image_generation_limit', message: 'Image generation reached its usage limit.', retryable: false,
      window: 'imagegen_premium', resets_at: RESET };
    const limited: Item = { ...errorItem('image-limit', payload.message, { turn: 'image-turn', at: AT }), kind: 'error', payload };
    await emit(ACME.mainThread, {
      thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('image-prompt', 'Make an orchard picture', { author: ADA, turn: 'image-turn', inputId: 'image-input', at: AT }), limited,
        toolCall('image-result', { kind: 'image', blob: HASH, media_type: 'image/png', name: NAME, caption: CAPTION },
          { title: NAME, turn: 'image-turn', at: AT })],
      turns: [turn('image-turn', { ordinal: 1, state: 'completed', inputId: 'image-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('image-input', 'Make an orchard picture', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'image-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: HASH, size: Buffer.from(PIXEL, 'base64').length, media_type: 'image/png', is_text: false,
      image: `data:image/png;base64,${PIXEL}`, text: null, width: 1, height: 1, duration_seconds: null, entry_count: null };
    const blobPath = `/api/v1/threads/${ACME.mainThread}/blobs/${HASH}`;
    await page.route((url) => url.pathname === blobPath, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else await route.fulfill({ contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') });
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
    const notice = page.locator('[data-item="image-limit"]');
    await expect(notice.getByRole('alert')).toContainText(words.ui.error.imageGenerationLimit);
    await expect(notice).toContainText(words.timeline.failure.reportedLimit.replace('{{limit}}', 'imagegen_premium'));
    await expect(notice.locator('time')).toHaveAttribute('datetime', RESET);
    await expect(page.locator('[data-thread-header]')).toContainText(words.state.idle);
    const image = page.locator('[data-step="image-result"]');
    await image.locator('[data-tool-row] > button[aria-expanded]').click();
    await expect(image.getByText(CAPTION, { exact: true })).toBeVisible();
    const open = image.getByRole('button', { name: words.fileMedia.openFile.replace('{{name}}', NAME), exact: true });
    await open.focus();
    await page.keyboard.press('Enter');
    const viewer = page.getByRole('dialog', { name: NAME, exact: true });
    await expect(viewer.locator('[data-viewer="image"] img')).toBeVisible();
    await viewer.getByRole('banner').getByRole('button', { name: words.common.close, exact: true }).click();
    await expect(viewer).toHaveCount(0);
    await expect(open).toBeFocused();
    await page.reload();
    await expect(page.locator('[data-item="image-limit"] time')).toHaveAttribute('datetime', RESET);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`image-generation-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('image-generation', { path: screenshot, contentType: 'image/png' });
  });

  test(`a reader opens the selected program's image explanation ${suffix}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const def: SettingDef = { key: KEY, title: 'Image generation', description: "Whether the agent program's native image generation is enabled. Unset keeps the program's own default.",
      group: 'agent', class: 'parameter', scopes: ['system', 'org', 'project', 'topic', 'thread'], merge: 'replace', default: null, example: 'on',
      schema: { anyOf: [{ type: 'string', enum: ['on', 'off'] }, { type: 'null' }] }, text_key: null, keywords: [] };
    const current: ResolvedSetting = { key: KEY, value: null, own: null, source: { scope: 'default', node: null }, locked_by: null,
      writable: false, parts: [], not_applicable_reason: 'harness_image_generation_unavailable:Claude Code' };
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [def] }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: [current] }));
    await supplyDefaults(page, ACME.research, { registry: () => [def], settings: () => [current] });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto('/org/acme.research/settings/defaults');
    const row = page.locator(`[data-setting="${KEY}"]`);
    await expect(row).toContainText(words.settingRegistry.agents.image_generation.title);
    await row.getByRole('button', { name: words.help.hint.label.replace('{{term}}', words.settingRegistry.agents.image_generation.title), exact: true }).click();
    await expect(page.getByText(words.inspector.parameters.imageGenerationUnsupported.replace('{{program}}', 'Claude Code'), { exact: true })).toBeVisible();
    await expect(row.getByRole('combobox')).toHaveCount(0);
    await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  const words = locale === 'en' ? english : chinese;
  test(`a paid image stays viewable when file delivery fails ${locale}`, async ({ page }) => {
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width: 400, height: 900 });
    const delivery = '60000000-0000-4000-8000-000000000020';
    const images = [1, 2].map((index) => ({ blob: HASH, item_id: `image-paid-${index}`, path: `out/picture-${index}.png`, status: 'failed', size: Buffer.from(PIXEL, 'base64').length }));
    await emit(ACME.mainThread, {
      thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('paid-image-prompt', 'Make two orchard pictures', { author: ADA, turn: 'paid-image-turn', inputId: 'paid-image-input', at: AT }),
        toolCall('paid-image-input-pixels', { kind: 'image', blob: HASH, media_type: 'image/png', name: 'source-1', caption: null }, { title: 'Image input', turn: 'paid-image-turn', at: AT }),
        toolCall('paid-image-call', { kind: 'orch', tool: 'image_generation', arguments: { prompt: 'Two orchards', output_paths: images.map((image) => image.path) }, result: { delivery_ref: delivery, images } }, { title: 'image_generation', turn: 'paid-image-turn', at: AT }),
        ...images.map((image, index) => toolCall(image.item_id, { kind: 'image', blob: image.blob, media_type: 'image/png', name: `image-${index + 1}`, caption: CAPTION }, { title: 'Generated image', turn: 'paid-image-turn', at: AT }))],
      turns: [turn('paid-image-turn', { ordinal: 1, state: 'completed', inputId: 'paid-image-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('paid-image-input', 'Make two orchard pictures', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'paid-image-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: HASH, size: Buffer.from(PIXEL, 'base64').length, media_type: 'image/png', is_text: false,
      image: `data:image/png;base64,${PIXEL}`, text: null, width: 1, height: 1, duration_seconds: null, entry_count: null };
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${HASH}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else await route.fulfill({ contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/thread/${ACME.mainThread}`);
    const call = page.locator('[data-step="paid-image-call"]');
    await call.locator('[data-tool-row] > button[aria-expanded]').click();
    const result = call.getByRole('list', { name: words.timeline.mcp.result, exact: true });
    await expect(result).toContainText(delivery);
    await result.getByRole('button', { name: words.ui.json.expand.replace('{{name}}', `${words.timeline.mcp.result}.images.0`), exact: true }).click();
    await expect(result).toContainText('failed');
    const source = page.locator('[data-step="paid-image-input-pixels"]');
    await source.locator('[data-tool-row] > button[aria-expanded]').click();
    const sourceOpen = source.getByRole('button', { name: words.fileMedia.openFile.replace('{{name}}', 'source-1'), exact: true });
    await sourceOpen.click();
    const sourceViewer = page.getByRole('dialog', { name: 'source-1', exact: true });
    await expect(sourceViewer.locator('[data-viewer="image"] img')).toBeVisible();
    await sourceViewer.getByRole('banner').getByRole('button', { name: words.common.close, exact: true }).click();
    await expect(sourceOpen).toBeFocused();
    for (let index = 1; index <= 2; index += 1) {
      const image = page.locator(`[data-step="image-paid-${index}"]`);
      await image.locator('[data-tool-row] > button[aria-expanded]').click();
      await expect(image.getByText(CAPTION, { exact: true })).toBeVisible();
      const open = image.getByRole('button', { name: words.fileMedia.openFile.replace('{{name}}', `image-${index}`), exact: true });
      await open.focus();
      await page.keyboard.press('Enter');
      const viewer = page.getByRole('dialog', { name: `image-${index}`, exact: true });
      await expect(viewer.locator('[data-viewer="image"] img')).toBeVisible();
      await viewer.getByRole('banner').getByRole('button', { name: words.common.close, exact: true }).click();
      await expect(open).toBeFocused();
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.reload();
    await expect(page.locator('[data-step="image-paid-1"]')).toBeVisible();
    await expect(page.locator('[data-step="image-paid-2"]')).toBeVisible();
  });
}
