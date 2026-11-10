/** An explicitly opened off Links view gives its native state and the existing configuration control. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { DocumentLinks, DocumentSync } from '../../src/generated/api-types';
import { DOCS, openDocument } from './documents-support';
import { scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`off document links ${locale}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    let locked = false;
    const linkSettings = (): DocumentLinks => ({ shown: [], own: [], inherited: ['referenced_by'], locked, writable: !locked });
    // The joined document opening seeds Links before its separate reader is needed.
    await page.route(`**/api/v1/documents/${DOCS.plan}/sync*`, async (route) => {
      const response = await route.fetch();
      const data = await response.json() as DocumentSync;
      data.document.link_settings = linkSettings();
      await route.fulfill({ response, json: data });
    });
    await page.route(`**/api/v1/documents/${DOCS.plan}/links`, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      await route.fulfill({ json: linkSettings() });
    });
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await openDocument(page, DOCS.plan);
      await showWorkbenchView(page, 'Links');
      await expect(page.getByText(words.implicitAvailability.linksOff, { exact: true })).toBeVisible();
      const configure = page.getByRole('button', { name: words.links.dialog.title, exact: true });
      if (!locked) {
        await configure.click();
        const dialog = page.getByRole('dialog', { name: words.links.dialog.title, exact: true });
        await expect(dialog.getByRole('checkbox')).toHaveCount(4);
        await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
        locked = true;
        await page.reload();
        await showWorkbenchView(page, 'Links');
      }
      await expect(configure).toHaveAttribute('aria-disabled', 'true');
      await expect(configure).toHaveAttribute('aria-description', words.links.dialog.locked);
      await info.attach(`document-links-availability-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
