/** The shipped layout hint reaches its authored section in both languages and frame sizes. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { READING_GUIDE, readingGuideHeading } from '../../src/help/reading.ts';
import { replySent, scenario, signIn } from './support';

for (const width of [400, 1440]) for (const locale of ['en', 'zh-CN'] as const) {
  test(`the reading hint opens its actual section (${width}/${locale})`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    const signedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth: unknown = await signedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The scenario returned no native sign-in');
    const saved = await page.request.patch('/api/v1/me', { headers: { Authorization: 'Bearer ' + auth.access_token }, data: { locale } });
    expect(saved.ok()).toBe(true);
    await page.goto('/org/acme');
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await page.getByRole('button', { name: caption('@workbench.arrangements.label', locale), exact: true }).click();
    const entry = page.getByRole('menuitem', { name: caption('@readingGuide.cards', locale), exact: true });
    await expect(entry).toHaveAttribute('data-help-article', READING_GUIDE.id);
    await expect(entry).toHaveAttribute('data-help-heading', readingGuideHeading(locale, 'cards'));
    await entry.click();
    await expect(page.getByRole('heading', { name: caption('@readingGuide.title', locale), exact: true })).toBeVisible();
    await expect(page.locator(`h2[id="${readingGuideHeading(locale, 'cards')}"]`)).toBeInViewport();
    if (width === 400) await expect(page).toHaveURL(new RegExp('/help/' + READING_GUIDE.id));
    else await expect(page).toHaveURL(/\/org\/acme(?:\?|$)/);
  });
}
