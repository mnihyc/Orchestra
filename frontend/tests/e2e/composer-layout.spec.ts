/** Composer guidance and the active phone sheet follow the same click path in both authored languages. */
import { expect, test } from '@playwright/test';
import { openLayoutRoute, prepareLayoutRoute, resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { caption } from '../../e2e/capture/open.ts';
import { ACME, scenario } from './support.ts';

for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`composer guidance and covered Parameters controls ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await resourceFixtures(page);
    await signInScreenRoute(page, { width, theme, locale });
    await page.reload();
    const words = (key: string) => caption(`@${key}`, locale);
    const row = { pattern: '/thread/$threadId', url: `/thread/${ACME.mainThread}`, fixture: 'background-running' } as const;
    await prepareLayoutRoute(page, row); await page.goto(row.url);
    const controls = page.locator('[data-composer-controls]'); await expect(controls).toBeVisible();
    const helps = page.locator('[data-help-article="paste-and-upload"][data-help-heading="message-files"]:visible');
    await info.attach('attachment-help', { body: JSON.stringify(await helps.evaluateAll((items) => items.map((item) => ({ name: item.getAttribute('aria-label'), parent: item.parentElement?.outerHTML })))), contentType: 'application/json' });
    await expect(helps).toHaveCount(1);
    await helps.click(); await expect(page.getByRole('dialog', { name: words('uploadHelp.messageTitle'), exact: true })).toBeVisible(); await page.keyboard.press('Escape');
    const parameters = { ...row, fixture: 'thread-options-allowed' } as const;
    await prepareLayoutRoute(page, parameters); await page.reload(); await openLayoutRoute(page, parameters);
    await expect(page.getByTestId('parameters-panel')).toBeVisible();
    if (width === 400) {
      await expect(page.locator('[data-mobile-main-tabs]')).toHaveAttribute('inert', '');
      await expect(page.locator('[data-mobile-main-tabs]')).toHaveCSS('visibility', 'hidden');
      await page.getByRole('button', { name: words('common.close'), exact: true }).click();
      await expect(page.locator('[data-mobile-main-tabs]')).not.toHaveAttribute('inert', '');
      await expect(page.locator('[data-mobile-main-tabs]')).toHaveCSS('visibility', 'visible');
    }
    const sharing = { ...row, fixture: 'agent-share-ladders' } as const;
    await prepareLayoutRoute(page, sharing); await page.goto(sharing.url); await openLayoutRoute(page, sharing);
    await expect(page.locator('[data-help-article="paste-and-upload"][data-help-heading="message-files"]:visible')).toHaveCount(0);
    await page.getByRole('combobox', { name: words('access.agents.dialog.whose'), exact: true }).click();
    await expect(page.getByRole('listbox', { name: words('access.agents.dialog.whose'), exact: true })).toContainText(words('ui.combobox.minimumQuery_other').replace('{{count}}', '2'));
  });
}
