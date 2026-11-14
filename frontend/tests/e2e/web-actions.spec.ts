/** Search, open and find remain distinct through the real timeline and raw-source disclosure. */
import { expect, test } from '@playwright/test';
import type { Item } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { AT, rawOf, threadInput, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { ACME, configureThread, emit, scenario, signIn } from './support';

const ADDRESS = 'https://docs.example.invalid/guide';
type Action = { type: 'search' } | { type: 'open_page'; url: string | null } | { type: 'find_in_page'; url: string | null; pattern: string | null } | { type: 'other' };

function step(id: string, action: Action, nativeAction: object): Item {
  const tool = { kind: 'web_search' as const, action, query: 'orchards', results: action.type === 'search' ? [{ title: 'Orchard guide', url: ADDRESS }] : [] };
  const item = toolCall(id, tool, { title: 'Native web action', turn: 'web-turn', at: AT });
  if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
  const input = JSON.stringify({ action: nativeAction, query: 'orchards' });
  return { ...item, payload: { ...item.payload, raw: { ...rawOf(item), name: 'webSearch',
    input: { inline: input, total_len: new TextEncoder().encode(input).length, truncated: false, blob: null } } } };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`web action titles and explicit page links keep their meaning ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await configureThread(ACME.mainThread, { reset: true });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null },
      items: [userMessage('web-input', 'Read the orchard guide', { author: ADA, turn: 'web-turn', inputId: 'web-input', at: AT }),
        step('search-web', { type: 'search' }, { type: 'search', query: 'orchards' }),
        step('open-page', { type: 'open_page', url: ADDRESS }, { type: 'openPage', url: ADDRESS }),
        step('find-page', { type: 'find_in_page', url: ADDRESS, pattern: 'harvest' }, { type: 'findInPage', url: ADDRESS, pattern: 'harvest' }),
        step('unreported-page', { type: 'open_page', url: null }, { type: 'openPage', url: null }),
        step('future-web', { type: 'other' }, { type: 'futureAction' })],
      turns: [turn('web-turn', { ordinal: 1, state: 'completed', inputId: 'web-input', startedAt: AT, completedAt: AT })],
      inputs: [{ ...threadInput('web-input', 'Read the orchard guide', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'web-turn', delivered_at: AT }],
    });
    let visits = 0;
    await page.context().route(ADDRESS, async (route) => { visits += 1; await route.fulfill({ contentType: 'text/html', body: '<title>Orchard guide</title><p>The harvest guide</p>' }); });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByText('Read the orchard guide', { exact: true })).toBeVisible();
    const fold = page.locator('[data-fold="web-turn"]');
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(page.locator('[data-step="search-web"]')).toContainText(words.timeline.step.webSearch.replace('{{query}}', 'orchards'));
    const open = page.locator('[data-step="open-page"]');
    await expect(open).toContainText(words.timeline.step.webOpen.replace('{{address}}', 'docs.example.invalid/guide'));
    await expect(page.locator('[data-step="find-page"]')).toContainText(words.timeline.step.webFind.replace('{{pattern}}', 'harvest'));
    await expect(page.locator('[data-step="future-web"]')).toContainText(words.timeline.step.webAction);
    await open.locator('[data-tool-row] > button[aria-expanded]').click();
    await expect(open).not.toContainText(words.timeline.web.noResults);
    await open.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    await expect(open.locator('[data-raw="open-page"]')).toContainText('openPage');
    await open.getByRole('button', { name: words.timeline.step.showDisplay, exact: true }).click();
    const missing = page.locator('[data-step="unreported-page"]');
    await missing.locator('[data-tool-row] > button[aria-expanded]').click();
    await expect(missing).toContainText(words.timeline.web.noAddress);
    await expect(missing.getByRole('link')).toHaveCount(0);
    expect(visits).toBe(0);
    const popup = page.waitForEvent('popup');
    await open.getByRole('link').click();
    const guide = await popup;
    await expect(guide).toHaveTitle('Orchard guide');
    expect(visits).toBe(1);
    await guide.close();
    await page.reload();
    await expect(page.getByText('Read the orchard guide', { exact: true })).toBeVisible();
    if (await fold.count() > 0 && await fold.first().getAttribute('aria-expanded') === 'false') await fold.first().click();
    await expect(page.locator('[data-step="find-page"]')).toContainText(words.timeline.step.webFind.replace('{{pattern}}', 'harvest'));
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`web-actions-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('web-actions', { path: screenshot, contentType: 'image/png' });
  });
}
