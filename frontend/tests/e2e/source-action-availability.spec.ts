/** Source links use their real inspector entrance and keep a permission refusal visible. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { MAIN } from './change-support';
import { deny, PEOPLE, scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`source link availability ${locale}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/thread/${MAIN}`);
      await showWorkbenchView(page, 'Parameters');
      const links = page.getByRole('region', { name: words.source.links.list, exact: true });
      const edit = links.getByRole('button', { name: words.source.links.edit, exact: true });
      if (width === 1280) {
        await edit.click();
        const dialog = page.getByRole('dialog', { name: words.source.links.title, exact: true });
        await dialog.getByRole('button', { name: words.source.links.another, exact: true }).click();
        await dialog.getByRole('textbox', { name: words.source.links.label, exact: true }).last().fill('Draft link');
        await dialog.getByRole('textbox', { name: words.source.links.url, exact: true }).last().fill('https://example.test/review');
        await expect(dialog.getByRole('textbox', { name: words.source.links.url, exact: true }).last()).toHaveValue('https://example.test/review');
        await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
        await deny(PEOPLE.ada.login, ['thread.interact']);
        await page.reload();
        await showWorkbenchView(page, 'Parameters');
      }
      await expect(edit).toHaveAttribute('aria-disabled', 'true');
      await edit.focus();
      await expect(page.getByRole('tooltip')).toContainText(words.implicitAvailability.threadInteract);
      const bounds = await links.boundingBox();
      if (bounds === null) throw new Error('the actual thread-links inspector line is visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`source-links-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      await edit.click();
      await expect(page.getByRole('dialog', { name: words.source.links.title, exact: true })).toHaveCount(0);
    }
  });
}
