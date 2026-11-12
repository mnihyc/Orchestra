/** Settings have one content scroller; a separate browser fixture proves the sweep catches nested scrolling. */
import { expect, test } from '@playwright/test';
import { expectOneScrollerPerArea, scrollingFaults } from '../accessibility/scrolling';
import { scenario } from './support';
import { signInForManagement } from './admin-mode-support';

test('Management Settings keeps the pane still and only its view scrolls', async ({ page }) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await signInForManagement(page);
  await page.goto('/admin/settings');
  await expect(page.locator('[data-management-content]')).toBeVisible();
  await expectOneScrollerPerArea(page);
  const content = page.locator('[data-management-content]');
  await expect(content.locator('[data-setting-row]').first()).toBeVisible();
  // Writable settings now have compact typed summaries inside their edit controls;
  // raw inline previews no longer exist here. The shared view owns the scroll.
  // Use the measured content height so viewport chrome leaves an actual overflow.
  const contentHeight = await content.evaluate((element) => element.scrollHeight);
  await page.setViewportSize({ width: 1440, height: contentHeight });
  await expectOneScrollerPerArea(page);
  const scrollers = await page.locator('[data-view-group]').filter({ has: page.locator('[data-management-content]') }).evaluate((area) => Array.from(area.querySelectorAll<HTMLElement>('*')).filter((element) => {
    const style = getComputedStyle(element);
    return /^(auto|scroll)$/.test(style.overflowY) && element.scrollHeight > element.clientHeight + 1 && element.getClientRects().length > 0;
  }).filter((element) => element.contains(area.querySelector('[data-management-content]')) || element.hasAttribute('data-management-content')).length);
  expect(scrollers).toBe(1);
  const owner = await content.evaluateHandle((element) => {
    for (let at: Element | null = element; at !== null; at = at.parentElement) {
      if (/^(auto|scroll)$/.test(getComputedStyle(at).overflowY) && at.scrollHeight > at.clientHeight + 1) return at;
    }
    throw new Error('The settings view has a measured scroll owner');
  });
  await owner.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect.poll(() => owner.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await expectOneScrollerPerArea(page);
  await owner.dispose();
});

test('the probe detects nested scrollers and permits explicitly declared code content', async ({ page }) => {
  // This detector fixture intentionally has no API; the screen case above uses the fake world.
  await page.setContent('<main data-view-group style="height:100px;overflow:auto"><div style="height:200px"><div id="inner" style="height:50px;overflow:auto"><pre style="height:200px">long code</pre></div></div></main>');
  expect(await scrollingFaults(page)).toContain('y: div#inner scrolls inside main');
  await page.locator('#inner').evaluate((element) => { element.dataset.scrollAllow = 'code'; });
  await expectOneScrollerPerArea(page);
});
