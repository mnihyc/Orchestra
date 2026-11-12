/** Own click path for the shared resolver's native-image projection, in both languages and themes. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`native image eligibility stays a supplied fact ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    // The Rust declaration tests decide actual auth/plan eligibility. This is its public wire projection.
    let allowed = false;
    const capabilities = () => [{ capability: 'image_generation', title: 'Image generation', program_default: true, provider: null, prices: {},
      reason: allowed ? null : 'program_support_not_applicable', detail: allowed ? null : 'native_image_sign_in_required',
      route_view: allowed ? { kind: 'native-program', id: 'image_generation.program', disable_native: false, orch_tool: null } : null }];
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as Record<string, unknown>;
      await route.fulfill({ response, json: { ...original, capabilities: capabilities() } });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening?*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: Record<string, unknown> | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : { ...original.parameters, capabilities: capabilities() } } });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${MAIN}`);
    await showView(page, words.views.parameters.title);
    const row = page.locator('[data-capability="image_generation"]');
    await expect(row).toContainText(words.capabilityRoutes.reasons.native_image_sign_in_required);
    await expect(row.getByRole('combobox')).toHaveCount(0);
    allowed = true;
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(row).toContainText(words.capabilityRoutes.routes['native-program']);
    await expect(row).toContainText(words.capabilityRoutes.programDefault);
    await expect(row).not.toContainText(words.capabilityRoutes.routes.service);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('native-image-eligibility', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
