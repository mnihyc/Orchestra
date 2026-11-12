/** A tree selection offers only eligible targets and never opens an empty sharing action. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { inventoryRow } from '../doubles/everything-row';
import { id } from '../doubles/fixtures';
import { DOUBLE, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`selection sharing eligibility ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    const configured = await fetch(`${DOUBLE}/__double/everything/rows`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username: 'ada', rows: [
      inventoryRow({ id: id(49, 91), kind: 'document', name: 'Eligible document selection', can_share: true, href: '/doc/eligible' }),
      inventoryRow({ id: id(49, 92), kind: 'thread', name: 'Readonly thread selection', can_share: false, href: '/thread/readonly' }),
    ] }) });
    expect(configured.ok).toBe(true);
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      // A name query returns these loose fixture rows directly, outside the root-org hierarchy.
      await page.goto('/everything?q=selection');
      await page.getByRole('button', { name: words.everything.select, exact: true }).click();
      const eligible = page.locator('[data-everything-row]').filter({ hasText: 'Eligible document' }).getByRole('checkbox');
      const readonly = page.locator('[data-everything-row]').filter({ hasText: 'Readonly thread' });
      await expect(readonly).toBeVisible();
      // The inventory now excludes ineligible targets from selection, so the former mixed subset cannot occur here.
      await expect(readonly.getByRole('checkbox')).toHaveCount(0);
      await eligible.check();
      const selection = page.getByRole('region', { name: words.everything.selection, exact: true });
      await expect(selection.getByRole('status')).toHaveCount(0);
      await expect(selection.getByRole('button', { name: words.everything.share, exact: true })).not.toHaveAttribute('aria-disabled', 'true');
      await eligible.uncheck();
      const share = selection.getByRole('button', { name: words.everything.share, exact: true });
      await expect(share).toHaveAttribute('aria-disabled', 'true');
      await expect(share).toHaveAttribute('aria-description', words.implicitAvailability.noneSelected);
      await share.press('Enter');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      const bounds = await selection.boundingBox();
      if (bounds === null) throw new Error('the actual selection toolbar is visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`sharing-subset-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
