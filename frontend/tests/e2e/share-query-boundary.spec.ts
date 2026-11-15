/** Share explains its query boundary while retaining its owners, roster and native person search. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, configureThread, scenario } from './support.ts';

for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`Share keeps the roster while its person query is empty ${width}/${theme}/${locale}`, async ({ page }) => {
    await scenario(); await configureThread(ACME.mainThread, { reset: true }); await signInScreenRoute(page, { width, theme, locale });
    await page.goto(`/thread/${ACME.mainThread}`);
    const words = (key: string) => caption(`@${key}`, locale);
    await page.locator('[data-thread-header] [data-thread-more]').click();
    await page.getByRole('menuitem', { name: words('thread.header.shareThread'), exact: true }).click();
    const input = page.getByRole('combobox', { name: words('access.add.label'), exact: true });
    await expect(input).toBeFocused(); await expect(input).toHaveAttribute('aria-expanded', 'true');
    const list = page.getByRole('listbox', { name: words('access.add.label'), exact: true });
    const instruction = words('ui.combobox.minimumQuery_one').replace('{{count}}', '1');
    await expect(list).toContainText(instruction); await expect(list.getByRole('option')).toHaveCount(0);
    await expect(page.getByRole('group', { name: words('access.owners.label'), exact: true })).toContainText('Ada Lovelace');
    await expect(page.getByRole('region', { name: words('access.roster.title'), exact: true })).toBeVisible();
    const changes: string[] = []; page.on('request', (request) => { if (request.method() !== 'GET' && new URL(request.url()).pathname.endsWith('/permissions')) changes.push(request.method()); });
    await input.fill('Grace'); await expect(list.getByRole('option', { name: 'Grace Hopper · acme.grace', exact: true })).toBeVisible();
    await input.fill(''); await expect(list).toContainText(instruction); await expect(list.getByRole('option')).toHaveCount(0);
    expect(changes).toEqual([]);
  });
}
