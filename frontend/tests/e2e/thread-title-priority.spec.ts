/** The head of a thread is one row that never wraps: the title keeps its room, and the acts that do not fit leave the row, the last first, into the menu. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { turn } from '../doubles/thread-rows.ts';
import { ACME, configureThread, emit, scenario } from './support.ts';

for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`the thread head is one row, with the title cut and the rest in the menu ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await configureThread(ACME.mainThread, { reset: true });
    await emit(ACME.mainThread, { thread: { name: 'A thread of the world', state: 'running', current_turn_id: 'header-turn' },
      turns: [turn('header-turn', { ordinal: 1, state: 'running', inputId: 'header-input', startedAt: '2026-11-06T10:00:00Z' })] });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]'), heading = header.getByRole('heading', { level: 1 });
    await expect(heading).toHaveText('A thread of the world');
    const name = 'A thread of the world with the complete title kept before its controls';
    await emit(ACME.mainThread, { thread: { name } });
    await expect(heading).toHaveText(name);
    // The whole name is under the pointer; the row stays one row, the height of a control, however long the name is.
    await expect(heading).toHaveAttribute('title', name);
    const frame = await header.boundingBox();
    expect(frame?.height ?? Infinity).toBeLessThanOrEqual(40);
    for (const part of await header.locator('[data-fold]:visible, [data-fold-fixed]').all()) {
      const box = await part.boundingBox();
      expect(box?.y ?? -1).toBeGreaterThanOrEqual((frame?.y ?? 0) - 1);
      expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual((frame?.y ?? 0) + (frame?.height ?? 0) + 1);
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(width);
    }
    await header.locator('[data-thread-more]').click();
    for (const key of ['duplicate.now', 'duplicate.to', 'change.menu.switch', 'search.find.command', 'thread.bell.label', 'places.star.star', 'thread.header.shareThread']) {
      await expect(page.getByRole('menuitem', { name: caption(`@${key}`, locale), exact: true })).toHaveCount(1);
    }
    await page.getByRole('menuitem', { name: caption('@thread.header.shareThread', locale), exact: true }).click();
    await expect(page.getByRole('combobox', { name: caption('@access.add.label', locale), exact: true })).toBeVisible();
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
    await info.attach('thread-title', { body: await header.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
  });
}
