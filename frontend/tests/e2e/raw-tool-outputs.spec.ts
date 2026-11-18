/** Whole retained tool source is opened by the ordinary authorized file card, never on render. */
import { expect, test } from '@playwright/test';
import type { FilePreview } from '../../src/features/files/source';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, rawOf, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';

const HASH = 'd'.repeat(64);
const WHOLE = 'Complete native tool result preserved beyond its inline excerpt';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a retained raw output opens in the existing viewer ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const call = toolCall('retained-call', { kind: 'mcp', server: 'gallery', tool: 'render', arguments: {}, result: 'An image was returned', error: null },
      { title: 'gallery: render', turn: 'raw-turn', at: AT });
    if (call.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    const item = { ...call, payload: { ...call.payload, raw: { ...rawOf(call),
      output: { inline: '', total_len: Buffer.byteLength(WHOLE), truncated: true, blob: HASH } } } };
    await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('raw-input', 'Keep the original result', { author: ADA, turn: 'raw-turn', inputId: 'raw-input', at: AT }), item],
      turns: [turn('raw-turn', { ordinal: 1, state: 'completed', inputId: 'raw-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('raw-input', 'Keep the original result', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'raw-turn', delivered_at: AT }],
    });
    const preview: FilePreview = { state: 'ready', hash: HASH, size: Buffer.byteLength(WHOLE), media_type: 'text/plain', is_text: true,
      image: null, text: WHOLE, width: null, height: null, duration_seconds: null, entry_count: null };
    let fullReads = 0;
    await page.route((url) => url.pathname === `/api/v1/threads/${ACME.mainThread}/blobs/${HASH}`, async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: preview });
      else { if (route.request().method() === 'GET') fullReads += 1; await route.fulfill({ contentType: 'text/plain', body: WHOLE }); }
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByText('Keep the original result', { exact: true })).toBeVisible();
    const fold = page.locator('[data-fold="raw-turn"]');
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    const step = page.locator('[data-step="retained-call"]');
    const showRaw = async () => {
      const details = step.locator('[data-tool-row] > button[aria-expanded]');
      if (await details.getAttribute('aria-expanded') === 'false') await details.click();
      await expect(details).toHaveAttribute('aria-expanded', 'true');
      const raw = step.getByRole('button', { name: words.timeline.step.showRaw, exact: true })
        .or(step.getByRole('button', { name: words.timeline.step.showDisplay, exact: true }));
      if (await raw.getAttribute('aria-pressed') !== 'true') await raw.click();
      await expect(raw).toHaveAttribute('aria-pressed', 'true');
    };
    await showRaw();
    const whole = step.getByRole('button', { name: words.timeline.raw.fullOutput, exact: true });
    await expect(whole).toBeVisible();
    await expect(step).not.toContainText(words.timeline.raw.cut);
    expect(fullReads).toBe(0);
    await whole.click();
    const card = page.locator('[data-file-card]').filter({ hasText: words.timeline.raw.fullOutput });
    await card.getByRole('button', { name: words.fileMedia.open, exact: true }).click();
    const viewer = page.getByRole('dialog', { name: words.timeline.raw.fullOutput, exact: true });
    await expect(viewer.locator('[data-viewer="code"]')).toContainText(WHOLE);
    expect(fullReads).toBeGreaterThan(0);
    await page.keyboard.press('Escape');
    await expect(viewer).toHaveCount(0);
    await expect(whole).toBeFocused();
    await page.reload();
    await expect(page.getByText('Keep the original result', { exact: true })).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await showRaw();
    await expect(step.getByRole('button', { name: words.timeline.raw.fullOutput, exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`raw-tool-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('raw-tool-output', { path: screenshot, contentType: 'image/png' });
  });
}
