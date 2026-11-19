/** The actual workbench, lists, search and dialogs share quiet chrome in both languages and themes. */
import { expect, test } from '@playwright/test';

import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { expectVisualChrome } from './chrome-geometry';
import { ACME, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`one visual chrome scale (${locale}, ${theme})`, async ({ page }, info) => {
    // Thirty-two route visits and four overlay scenes: five seconds per scene is a bounded suite budget.
    test.setTimeout(180_000);
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await page.setViewportSize({ width: 1440, height: 900 });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    const words = locale === 'en' ? english : chinese;
    const paths = [
      '/org/acme', `/project/${ACME.web}`, `/topic/${ACME.fixLogin}`, `/thread/${ACME.mainThread}`,
      '/resources/machines', '/resources/providers', '/resources/agents', '/resources/environments',
      '/resources/templates', '/resources/hosts', '/resources/mcp', '/resources/mounts',
      '/settings/profile', '/settings/interface', '/usage', '/search?q=login',
    ];
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      for (const [index, path] of paths.entries()) {
        await page.goto(path);
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
        await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
        if (path.startsWith('/resources/')) await expect(page.getByRole('table').first()).toBeVisible();
        await expectVisualChrome(page);
        await info.attach(`chrome-${index}-${width}-${locale}-${theme}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      }
      await page.goto('/resources/machines');
      const search = page.getByRole('searchbox', { name: words.machines.list.search });
      await expect(search).toBeVisible();
      await search.fill('laptop');
      await expect(search).toHaveValue('laptop');
      await expectVisualChrome(page);
      await search.fill('');
      await page.getByRole('button', { name: words.machines.list.add, exact: true }).click();
      const enrollment = page.getByRole('dialog', { name: words.machines.add.title, exact: true });
      await expect(enrollment).toBeVisible();
      await expectVisualChrome(page);
      await info.attach(`enrollment-${width}-${locale}-${theme}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      await page.keyboard.press('Escape');
      await expect(enrollment).toHaveCount(0);
      await page.keyboard.press('Control+k');
      const finder = page.getByRole('dialog', { name: words.finder.title, exact: true });
      await expect(finder).toBeVisible();
      const field = finder.getByRole('combobox');
      await field.fill('Web');
      await expect(field).toHaveValue('Web');
      await expectVisualChrome(page);
      await info.attach(`finder-${width}-${locale}-${theme}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      await page.keyboard.press('Escape');
      await expect(search).toBeVisible();
      if (width === 1440) {
        await page.goto('/org/acme');
        const tree = page.getByRole('searchbox', { name: words.navigation.filter.label });
        await tree.fill('Web');
        await expectVisualChrome(page);
        const clear = page.getByRole('button', { name: words.navigation.filter.clear, exact: true });
        await clear.focus();
        await page.keyboard.press('Enter');
        await expect(tree).toHaveValue('');
      }
    }
  });
}
