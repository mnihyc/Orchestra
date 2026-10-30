/** Settings have one content scroller; a separate browser fixture proves the sweep catches nested scrolling. */
import { expect, test } from '@playwright/test';
import { expectOneScrollerPerArea, scrollingFaults } from '../accessibility/scrolling';
import { scenario } from './support';
import { signInForManagement } from './admin-mode-support';

test('Management Settings keeps the pane still and only its view scrolls', async ({ page }) => {
  await scenario();
  await signInForManagement(page);
  await page.goto('/admin/settings');
  await expect(page.locator('[data-management-content]')).toBeVisible();
  await expectOneScrollerPerArea(page);
  // Read-only values expand on this scroller even when a code exception would allow another one.
  const previews = page.locator('[data-management-content] [data-content-preview]');
  await expect(previews.first()).toBeVisible();
  const all = previews.getByRole('button', { name: 'Show all', exact: true });
  if (await all.count() > 0) await all.first().click();
  const nestedValues = await page.locator('[data-management-content] pre').evaluateAll((values) => values.filter((value) => {
    const style = getComputedStyle(value);
    return /^(auto|scroll)$/.test(style.overflowY) && value.scrollHeight > value.clientHeight + 1;
  }).length);
  expect(nestedValues).toBe(0);
  await expectOneScrollerPerArea(page);
  const scrollers = await page.locator('[data-view-group]').filter({ has: page.locator('[data-management-content]') }).evaluate((area) => Array.from(area.querySelectorAll<HTMLElement>('*')).filter((element) => {
    const style = getComputedStyle(element);
    return /^(auto|scroll)$/.test(style.overflowY) && element.scrollHeight > element.clientHeight + 1 && element.getClientRects().length > 0;
  }).filter((element) => element.contains(area.querySelector('[data-management-content]')) || element.hasAttribute('data-management-content')).length);
  expect(scrollers).toBe(1);
  await page.locator('[data-management-content]').evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect.poll(() => page.locator('[data-management-content]').evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
});

test('the probe detects nested scrollers and permits explicitly declared code content', async ({ page }) => {
  // This detector fixture intentionally has no API; the screen case above uses the fake world.
  await page.setContent('<main data-view-group style="height:100px;overflow:auto"><div style="height:200px"><div id="inner" style="height:50px;overflow:auto"><pre style="height:200px">long code</pre></div></div></main>');
  expect(await scrollingFaults(page)).toContain('y: div#inner scrolls inside main');
  await page.locator('#inner').evaluate((element) => { element.dataset.scrollAllow = 'code'; });
  await expectOneScrollerPerArea(page);
});
