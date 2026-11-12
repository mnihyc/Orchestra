/** Current program summaries and grouped capabilities stay readable at narrow widths. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import type { ProgramDetail, ProgramSummary } from '../../src/generated/api-types';
import { signInForManagement } from './admin-mode-support';
import { scenario } from './support';

const group = { en: 'Research tools', 'zh-CN': '研究工具' };
const title = (index: number) => ({ en: `Research tool ${index}`, 'zh-CN': `研究工具 ${index}` });
const reason = { en: 'This build did not pass the research check.', 'zh-CN': '此版本未通过研究检查。' };

test('program facts and grouped declarations stay readable at 400px in both themes and languages', async ({ page }, info) => {
  await scenario();
  let summary: ProgramSummary | undefined;
  await page.route('**/api/v1/admin/programs?*', async (route) => {
    const response = await route.fetch();
    const value = await response.json() as { items: ProgramSummary[]; next_cursor: string | null };
    const original = value.items[0];
    if (original === undefined) throw new Error('The native program directory is required');
    summary = { ...original, harness: { kind: 'research', name: 'Research adapter' }, protocol: 'ACP' };
    await route.fulfill({ response, json: { items: [summary], next_cursor: null } });
  });
  await page.route('**/api/v1/admin/programs/research', async (route) => {
    const response = await route.fetch({ url: route.request().url().replace(/research$/u, 'codex') });
    const original = await response.json() as ProgramDetail;
    if (summary === undefined) throw new Error('Open the actual directory before its program');
    await route.fulfill({ response, json: { ...original, summary, capabilities: [{ id: 'research', title: group,
      items: Array.from({ length: 12 }, (_, index) => ({ id: `research-${index}`, title: title(index), available: index !== 11, reason: index === 11 ? reason : null })),
    }] } });
  });
  await page.route('**/api/v1/admin/programs/research/builds?*', (route) => route.fulfill({ json: { items: [], next_cursor: null } }));
  await signInForManagement(page);
  for (const locale of ['en', 'zh-CN'] as const) {
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = (key: string) => caption(`@${key}`, locale);
    for (const theme of ['light', 'dark'] as const) {
      await page.goto('/admin/harnesses');
      const account = page.getByRole('button', { name: words('shell.account.label'), exact: true })
        .or(page.getByRole('button', { name: words('shell.small.me'), exact: true }));
      await account.click();
      await page.getByRole('menuitemradio', { name: locale === 'zh-CN' ? theme === 'light' ? '浅色' : '深色' : theme === 'light' ? 'Light' : 'Dark', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await page.setViewportSize({ width: 400, height: 860 });
      const table = page.getByRole('table', { name: words('programList.title'), exact: true });
      // The program detail is remembered; return through its actual directory control for the next pass.
      const directory = page.getByRole('button', { name: words('programList.title'), exact: true });
      await expect(table.or(directory)).toBeVisible();
      if (!await table.isVisible()) await directory.click();
      const program = table.getByRole('link', { name: 'Research adapter', exact: true });
      await expect(program).toBeVisible();
      await expect(table).toContainText('ACP');
      await program.focus(); await expect(program).toBeFocused(); await page.keyboard.press('Enter');
      await expect(page).toHaveURL(/program=research/u);
      await page.getByRole('tab', { name: words('programList.capabilities'), exact: true }).click();
      const declarations = page.getByRole('region', { name: group[locale], exact: true });
      await expect(declarations.locator('[data-program-capability]')).toHaveCount(12);
      const last = declarations.locator('[data-program-capability="research-11"]');
      await expect(last).toContainText(title(11)[locale]);
      await expect(last).toContainText(words('programCapabilities.off'));
      const help = last.getByRole('button', { name: words('help.hint.label').replace('{{term}}', title(11)[locale]), exact: true });
      await help.focus(); await page.keyboard.press('Enter');
      const detail = page.getByRole('dialog', { name: title(11)[locale], exact: true });
      await expect(detail).toContainText(reason[locale]);
      const box = await detail.boundingBox(); expect(box).not.toBeNull();
      if (box !== null) { expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual(400); expect(box.height).toBeLessThanOrEqual(860); }
      await page.keyboard.press('Escape'); await expect(detail).toHaveCount(0); await expect(help).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await info.attach(`program-capabilities-${locale}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      // EN: Management → Agent programs → keyboard-open Research adapter → Capabilities → unavailable declaration's reason → Escape.
      // 中文：管理 → 代理程序 → 用键盘打开 Research adapter → 能力 → 查看不可用能力的原因 → Esc 返回。
    }
  }
});
