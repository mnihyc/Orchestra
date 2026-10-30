/** UA-11's real topic header keeps one compact state, a keyboard menu and one write per change. */
import { expect, test } from '@playwright/test';

import { chats } from './chat-support.ts';
import { ACME, bodySent, scenario, signIn } from './support.ts';

const WORDS = {
  en: { status: 'Status', open: 'Open', done: 'Done', changeOpen: 'Change status: Open', changeDone: 'Change status: Done' },
  'zh-CN': { status: '状态', open: '进行中', done: '已完成', changeOpen: '更改状态：进行中', changeDone: '更改状态：已完成' },
} as const;

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`the topic state chip works at ${width}px in ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    const writes: Promise<unknown>[] = [];
    page.on('request', (request) => {
      if (request.method() === 'PATCH' && new URL(request.url()).pathname === `/api/v1/topics/${ACME.upgradeDeps}`) {
        writes.push(bodySent(request));
      }
    });
    await page.goto(`/topic/${ACME.upgradeDeps}?tab=activity`);
    const words = WORDS[locale];
    let chip = page.getByRole('button', { name: words.changeOpen, exact: true });
    await expect(chip).toHaveText(words.open);
    await expect(page.getByRole('combobox', { name: words.status, exact: true })).toHaveCount(0);
    await expect(chip.locator('[data-status-chip]')).toHaveCount(1);
    const bounds = await chip.boundingBox();
    if (bounds === null) throw new Error('the state chip is not drawn');
    expect(bounds.height).toBeGreaterThanOrEqual(24);
    expect(bounds.height).toBeLessThanOrEqual(36);
    expect(bounds.width).toBeLessThan(160);
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.keyboard.press('Tab');
    await chip.focus();
    await expect(chip).toBeFocused();
    expect(await chip.evaluate((element) => getComputedStyle(element).outlineStyle)).not.toBe('none');
    await page.keyboard.press('Enter');
    const menu = page.getByRole('menu', { name: words.status });
    await expect(menu.getByRole('menuitemradio', { name: words.open, exact: true })).toHaveAttribute('aria-checked', 'true');
    await menu.getByRole('menuitemradio', { name: words.open, exact: true }).click();
    expect(writes).toHaveLength(0);
    await chip.click();
    await menu.getByRole('menuitemradio', { name: words.done, exact: true }).click();
    chip = page.getByRole('button', { name: words.changeDone, exact: true });
    await expect(chip).toHaveText(words.done);
    await expect(chip).toBeEnabled();
    expect(await Promise.all(writes)).toEqual([{ status: 'done', expected_revision: expect.any(Number) }]);
    await chip.focus();
    await page.keyboard.press('Enter');
    await expect(menu.getByRole('menuitemradio', { name: words.done, exact: true })).toHaveAttribute('aria-checked', 'true');
    await menu.getByRole('menuitemradio', { name: words.open, exact: true }).click();
    await expect(page.getByRole('button', { name: words.changeOpen, exact: true })).toHaveText(words.open);
    expect(writes).toHaveLength(2);
    expect((await Promise.all(writes))[1]).toEqual({ status: 'open', expected_revision: expect.any(Number) });
    await info.attach(`topic-status-${width}-${locale}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });
}
