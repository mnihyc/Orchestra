/** A person opens Search from the rail, widens deliberately, shares it and starts again in the current org. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { SIGNS } from '../../src/features/finder/modes';
import { id } from '../doubles/fixtures';
import { makePrivateDocument } from './documents-support';
import { ACME, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`current org, personal hits and shared search address in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario();
    const own = await makePrivateDocument('ada', id(2, 1), 'Canary personal note');
    const office = await makePrivateDocument('ada', ACME.org, 'Canary office note');
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto('/org/acme');
    await page.getByRole('button', { name: words.finder.open, exact: true }).click();
    const finder = page.getByRole('dialog', { name: words.finder.title });
    await finder.getByRole('combobox', { name: words.finder.field }).fill(`${SIGNS.conversations} Canary`);
    // The preview itself sends the selected root, before opening the full page.
    const scoped = page.waitForRequest((request) => new URL(request.url()).pathname.endsWith('/search') && new URL(request.url()).searchParams.get('scope') === ACME.org);
    await finder.getByRole('combobox', { name: words.finder.field }).fill(`${SIGNS.conversations} Canary note`);
    await scoped;
    await finder.getByRole('option', { name: words.finder.conversations.advancedQuery.replace('{{query}}', 'Canary note'), exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`org=${ACME.org}`));
    const results = page.getByRole('region', { name: words.search.results });
    await expect(results.locator(`a[href="/doc/${office}"]`).first()).toBeVisible();
    await expect(results.locator(`a[href="/doc/${own}"]`)).toHaveCount(0);
    const choice = page.getByRole('button', { name: new RegExp(`^${words.search.org.label}:`) });
    await choice.click(); await page.getByRole('menuitemradio', { name: words.search.org.all, exact: true }).click();
    await expect(page).toHaveURL(/org=all/);
    await expect(results.locator(`a[href="/doc/${own}"]`).first()).toBeVisible();
    await expect(results.locator('[data-search-org-heading]')).toHaveCount(2);
    const hits = results.locator('li').filter({ has: page.locator('[data-search-org]') });
    await expect(hits).toHaveCount(2);
    const shared = page.url(); await page.reload();
    await expect(page).toHaveURL(shared); await expect(results.locator(`a[href="/doc/${own}"]`).first()).toBeVisible();
    await page.goto('/search?q=Canary+note');
    await expect(page).toHaveURL(new RegExp(`org=${ACME.org}`));
    await expect(results.locator(`a[href="/doc/${own}"]`)).toHaveCount(0);
    await page.goto('/search?q=Canary+personal+note');
    const all = page.getByRole('button', { name: new RegExp(`^${words.search.org.searchAll}`) });
    await expect(all).toBeVisible(); await expect(all).toContainText('1'); await all.click();
    await expect(results.locator(`a[href="/doc/${own}"]`).first()).toBeVisible();
    await expect(results.locator('[data-search-org-heading]')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
