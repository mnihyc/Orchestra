/** Actual workbench borders resize the title and tabs without changing the viewport or their selected work. */
import { expect, test, type Locator } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { turn } from '../doubles/thread-rows.ts';
import { ACME, configureThread, emit, scenario } from './support.ts';

const widthOf = (element: Locator) => element.evaluate((node) => node.getBoundingClientRect().width);
const name = 'A thread of the world with its full name';
async function fullTitle(heading: Locator) {
  await expect(heading).toHaveText(name);
  await expect.poll(() => heading.evaluate((element) => element.scrollWidth <= element.clientWidth && element.scrollHeight <= element.clientHeight)).toBe(true);
}
async function selectedInside(list: Locator, selected: Locator) {
  await expect(selected).toHaveAttribute('aria-selected', 'true');
  await expect.poll(async () => {
    const room = await list.boundingBox(), tab = await selected.boundingBox();
    // One pixel admits fractional border rounding, as for the ordinary strip controls.
    return room !== null && tab !== null && tab.x >= room.x - 1 && tab.x + tab.width <= room.x + room.width + 1;
  }).toBe(true);
  const tops = await list.getByRole('tab').evaluateAll((tabs) => tabs.map((tab) => tab.getBoundingClientRect().top));
  expect(Math.max(...tops) - Math.min(...tops)).toBeLessThan(1);
}

for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`pane borders preserve the full thread title and selected topic tab ${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await configureThread(ACME.mainThread, { reset: true });
    await emit(ACME.mainThread, { thread: { name, state: 'running', current_turn_id: 'resized-turn' },
      turns: [turn('resized-turn', { ordinal: 1, state: 'running', inputId: 'resized-input', startedAt: '2026-11-06T10:00:00Z' })] });
    await signInScreenRoute(page, { width: 1440, theme, locale }); await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]'), heading = header.getByRole('heading', { level: 1 });
    const border = page.getByRole('separator', { name: caption('@workbench.sash.right', locale), exact: true });
    await fullTitle(heading); const initial = await widthOf(header);
    await border.press('End'); await expect.poll(() => widthOf(header)).toBeGreaterThan(initial);
    await fullTitle(heading);
    const wide = await widthOf(header), expandedFolds = await header.getAttribute('data-thread-folded');
    await border.press('Home'); await expect.poll(() => widthOf(header)).toBeLessThan(wide);
    await fullTitle(heading); await expect(border).toBeFocused();
    const compact = await widthOf(header);
    // Keep the inner row at the shared compact boundary: this is still a desktop cell.
    const targetWidth = await header.evaluate((element) => {
      const style = getComputedStyle(element), root = getComputedStyle(document.documentElement);
      const token = root.getPropertyValue('--breakpoint-sm').trim();
      const boundary = Number.parseFloat(token) * (token.endsWith('rem') ? Number.parseFloat(root.fontSize) : 1);
      return boundary + Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.paddingRight);
    });
    expect(targetWidth).toBeGreaterThan(0); expect(wide).toBeGreaterThan(targetWidth);
    const handle = await border.boundingBox(); if (handle === null) throw new Error('The native workbench border has no bounds.');
    const x = handle.x + handle.width / 2, y = handle.y + handle.height / 2;
    await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x + targetWidth - compact, y); await page.mouse.up();
    await expect.poll(async () => Math.abs(await widthOf(header) - targetWidth)).toBeLessThanOrEqual(1);
    await expect(header).toHaveAttribute('data-thread-folded', /view/); await fullTitle(heading);
    // Pointer dragging does not focus a native separator; its keyboard resize must keep focus.
    await border.press('ArrowRight'); await expect.poll(() => widthOf(header)).toBeGreaterThan(targetWidth);
    await fullTitle(heading); await expect(border).toBeFocused();
    await border.press('ArrowLeft'); await expect.poll(async () => Math.abs(await widthOf(header) - targetWidth)).toBeLessThanOrEqual(1);
    await expect(header).toHaveAttribute('data-thread-folded', /view/); await expect(border).toBeFocused();
    const folded = await widthOf(header);
    expect(page.viewportSize()?.width).toBe(1440);
    await header.screenshot({ path: info.outputPath('thread-header-after-resize.png') });
    await header.locator('[data-thread-more]').click();
    await page.getByRole('menuitem', { name: caption('@thread.header.shareThread', locale), exact: true }).click();
    await expect(page.getByRole('combobox', { name: caption('@access.add.label', locale), exact: true })).toBeVisible();
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
    await border.press('End'); await expect.poll(async () => Math.abs(await widthOf(header) - wide)).toBeLessThanOrEqual(1);
    await expect.poll(() => header.getAttribute('data-thread-folded')).toBe(expandedFolds); await fullTitle(heading);
    await page.setViewportSize({ width: 400, height: 900 }); await fullTitle(heading);
    await expect(header.locator('[data-thread-more]')).toBeVisible();

    await page.setViewportSize({ width: 1440, height: 900 }); await page.goto(`/topic/${ACME.fixLogin}?tab=activity`);
    const list = page.getByRole('tablist', { name: caption('@places.topic.sections', locale), exact: true });
    const activity = list.getByRole('tab', { name: caption('@places.topic.tabs.activity', locale), exact: true });
    await expect(list.getByRole('tab')).toHaveCount(7); await selectedInside(list, activity);
    await border.press('End'); const wideTabs = await widthOf(list);
    await border.press('Home'); await expect.poll(() => widthOf(list)).toBeLessThan(wideTabs);
    await selectedInside(list, activity); await expect(border).toBeFocused(); expect(page.viewportSize()?.width).toBe(1440);
    await border.press('End'); await expect.poll(async () => Math.abs(await widthOf(list) - wideTabs)).toBeLessThanOrEqual(1);
    await selectedInside(list, activity);
    await page.setViewportSize({ width: 400, height: 900 }); await selectedInside(list, activity);
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('activity');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('selected-tab-after-resize.png') });
    await info.attach('header-pane-widths', { body: JSON.stringify({ viewport: 1440, initial, wide, folded, compact, expandedFolds }), contentType: 'application/json' });
  });
}
