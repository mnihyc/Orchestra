/** The layout manifest's named screen states also pass the existing accessibility audit. */
import { test } from '@playwright/test';
import { layoutRouteName, SCREEN_GATE_ROUTES, SCREEN_PRESENTATIONS } from '../../e2e/layout-routes.ts';
import { openLayoutRoute, prepareLayoutRoute, resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { screenReady } from '../../e2e/layout-rules-helpers.ts';
import { expectAccessible, scenario } from '../e2e/support.ts';

for (const mode of SCREEN_PRESENTATIONS) {
  const { width, theme, locale } = mode;
  test.describe(`screen gate ${width}px ${theme} ${locale}`, () => {
    test.use({ viewport: { width, height: 900 }, colorScheme: theme, reducedMotion: 'reduce' });
    test.beforeEach(async ({ page }) => {
      await scenario(); await resourceFixtures(page); await signInScreenRoute(page, mode);
    });
    for (const row of SCREEN_GATE_ROUTES) test(layoutRouteName(row), async ({ page }) => {
      await prepareLayoutRoute(page, row); await page.goto(row.url);
      await screenReady(page, locale, theme); await openLayoutRoute(page, row);
      await expectAccessible(page, theme);
    });
  });
}
