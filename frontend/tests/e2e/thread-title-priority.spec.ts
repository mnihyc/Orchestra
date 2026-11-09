/** The pane gives the thread name room before folding optional header actions into its existing menu. */
import { expect, test, type Locator } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { turn } from '../doubles/thread-rows.ts';
import { ACME, configureThread, emit, scenario } from './support.ts';

async function paintedTitle(heading: Locator) {
  await expect.poll(() => heading.evaluate((element) => element.scrollWidth <= element.clientWidth && element.scrollHeight <= element.clientHeight)).toBe(true);
}
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`thread title precedes its header actions ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await configureThread(ACME.mainThread, { reset: true });
    await emit(ACME.mainThread, { thread: { name: 'A thread of the world', state: 'running', current_turn_id: 'header-turn' },
      turns: [turn('header-turn', { ordinal: 1, state: 'running', inputId: 'header-input', startedAt: '2026-11-06T10:00:00Z' })] });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]'), heading = header.getByRole('heading', { level: 1 });
    await expect(heading).toHaveText('A thread of the world'); await paintedTitle(heading);
    if (width === 1440) await header.getByRole('button', { name: caption('@actions.share.label', locale), exact: true }).focus();
    const name = 'A thread of the world with the complete title kept before its controls';
    await emit(ACME.mainThread, { thread: { name } }); await expect(heading).toHaveText(name); await paintedTitle(heading);
    if (width === 1440) {
      await expect(header).toHaveAttribute('data-thread-actions-mode', 'menu');
      await expect(header.locator('[data-thread-more]')).toBeFocused();
      await expect(header.locator('[data-thread-quick-actions]')).toHaveAttribute('inert', '');
      await expect(header.locator('[data-thread-primary-actions]')).toHaveAttribute('inert', '');
      await header.locator('[data-thread-more]').click();
      for (const key of ['duplicate.now', 'duplicate.to', 'change.menu.switch', 'search.find.command', 'thread.bell.label', 'places.star.star', 'actions.share.label']) {
        await expect(page.getByRole('menuitem', { name: caption(`@${key}`, locale), exact: true })).toHaveCount(1);
      }
      await page.getByRole('menuitem', { name: caption('@actions.share.label', locale), exact: true }).click();
    } else {
      await header.getByRole('button', { name: caption('@actions.share.label', locale), exact: true }).click();
    }
    await expect(page.getByRole('combobox', { name: caption('@access.add.label', locale), exact: true })).toBeVisible();
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
    await info.attach('thread-title', { body: await header.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
  });
}
