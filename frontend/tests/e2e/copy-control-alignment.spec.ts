/** A real wrapped VNC command line keeps its ordinary Copy control on the first text line. */
import { expect, test } from '@playwright/test';
import { inspectLayout, type LayoutInput } from '../../e2e/layout-rules-dom';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

const geometry: LayoutInput = { duplicateAllowList: [], rawValues: [], mode: 'icons', settle: true };
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`copy controls retain first-line alignment and feedback ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await scenario(); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const path = `/resources/environments/${RUNS_ON.workspace}?tab=vnc`;
    await page.goto(path);
    const copies = page.locator('[data-displays-view]').getByRole('button', { name: words.vnc.menu.copyLine, exact: true });
    await expect(copies.first()).toBeVisible();
    const aligned = async () => {
      const result = await page.evaluate(inspectLayout, geometry);
      expect(result.faults.filter((fault) => fault.rule === 'icon-centre' && fault.text === words.vnc.menu.copyLine)).toEqual([]);
      for (const copy of await copies.all()) {
        const bounds = await copy.boundingBox();
        expect(bounds?.height).toBeGreaterThanOrEqual(28);
        expect(bounds?.width).toBeGreaterThanOrEqual(28);
      }
    };
    await aligned();
    await copies.first().press('Enter');
    await expect(copies.first().getByRole('status')).toHaveText(words.common.copied);
    await aligned();
    await page.addInitScript(() => {
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
        writeText: () => Promise.reject(new DOMException('Test clipboard refusal', 'NotAllowedError')),
      } });
    });
    await page.reload(); await expect(copies.first()).toBeVisible();
    await copies.first().press('Enter');
    await expect(copies.first().getByRole('status')).toHaveText(words.common.copyRefused);
    await aligned();
    const screenshot = info.outputPath(`copy-control-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('copy-control', { path: screenshot, contentType: 'image/png' });
  });
}
