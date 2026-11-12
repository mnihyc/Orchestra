/** A native sub-agent name stays whole on the card before and after its lane opens. */
import { expect, test, type Locator } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { prepareLayoutRoute, resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { id } from '../doubles/fixtures.ts';
import { ACME, scenario } from './support.ts';

async function wholeTitle(title: Locator, button: Locator) {
  await expect.poll(() => title.evaluate((element) => ({ horizontal: element.scrollWidth > element.clientWidth, vertical: element.scrollHeight > element.clientHeight }))).toEqual({ horizontal: false, vertical: false });
  const firstLine = await title.evaluate((element) => element.getBoundingClientRect().top + Number.parseFloat(getComputedStyle(element).lineHeight) / 2);
  for (const icon of await button.locator('svg').all()) {
    const centre = await icon.evaluate((element) => { const box = element.getBoundingClientRect(); return box.top + box.height / 2; });
    const typography = await title.evaluate((element) => ({ titleLine: getComputedStyle(element).lineHeight, titleTop: element.getBoundingClientRect().top, buttonLine: element.parentElement === null ? null : getComputedStyle(element.parentElement).lineHeight, iconRowHeight: element.previousElementSibling?.getBoundingClientRect().height }));
    expect(Math.abs(centre - firstLine), JSON.stringify({ centre, firstLine, typography })).toBeLessThan(1);
  }
}
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`the complete sub-agent title stays on its card ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await resourceFixtures(page); await signInScreenRoute(page, { width, theme, locale });
    const row = { pattern: '/thread/$threadId', url: `/thread/${ACME.mainThread}`, fixture: 'background-held' } as const;
    await prepareLayoutRoute(page, row); await page.goto(row.url);
    const card = page.locator(`[data-step="${id(0x84, 4)}"]`);
    const disclosure = card.locator('[data-tool-row] > button[aria-expanded]');
    const title = disclosure.locator('[data-overflow-text]').first();
    await expect(title).toHaveText(caption('@timeline.step.subagent', locale).replace('{{name}}', 'Authentication and session reviewer'));
    await wholeTitle(title, disclosure);
    // Its leading disclosure icon stays clear of the floating Jump to latest control over the row's centre.
    await disclosure.locator('svg').first().click(); await expect(disclosure).toHaveAttribute('aria-expanded', 'true');
    await expect(card.locator('[data-conversation-lane="subagent"]')).toBeVisible();
    await wholeTitle(title, disclosure);
    await info.attach('sub-agent-card', { body: await card.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
  });
}
