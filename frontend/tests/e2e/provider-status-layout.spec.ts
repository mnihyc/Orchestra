/** Provider status words stay readable beside names and sharing facts in every presentation. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { auditPage, screenReady } from '../../e2e/layout-rules-helpers';
import { scenario } from './support';

for (const mode of SCREEN_PRESENTATIONS) {
  test(`provider status words fit (${mode.width}/${mode.theme}/${mode.locale})`, async ({ page }, info) => {
    await scenario();
    await resourceFixtures(page);
    await signInScreenRoute(page, mode);
    await page.goto('/resources/providers');
    await screenReady(page, mode.locale, mode.theme);
    const table = page.getByRole('table', { name: caption('@providers.list.label', mode.locale), exact: true });
    const resetWords = caption('@providers.phrase.limitResets', mode.locale).split('{{time}}');
    const prefix = resetWords[0]?.trim();
    if (prefix === undefined || prefix === '') throw new Error('The reset label must name its native limit');
    const limit = table.getByText(prefix, { exact: false }).first();
    await expect(limit).toBeVisible();
    await limit.hover();
    const audit = await auditPage(page, '[data-resource-hint="provider"],table [data-status-chip]');
    await page.screenshot({ path: info.outputPath('provider-status.png'), fullPage: true, animations: 'disabled' });
    // Keep the complete status readable in its own row.
    expect(audit.cuts).toEqual([]);
    expect(audit.faults).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
