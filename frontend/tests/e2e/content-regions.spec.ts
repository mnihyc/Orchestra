/** Keyboard scrolling and bounded previews on the actual Management adapter page. */
import { agentDescriptor } from '../doubles/agent-descriptor.ts';
import { expect, test } from '@playwright/test';
import type { HarnessKindInfo } from '../../src/generated/api-types.ts';
import { signInForManagement } from './admin-mode-support.ts';
import { scenario } from './support.ts';

const adapter: HarnessKindInfo = { kind: 'research', title: 'Research adapter', harness: { kind: 'research', name: 'Research adapter' }, program: null, builds: [], descriptor: {
  ...agentDescriptor('Research adapter', { kind: 'research' }, { fork_native: true, input_images: true, steer: true, memory: 'directory' }),
  ...Object.fromEntries(Array.from({ length: 12 }, (_, index) => [`field${index}`, { value: index }])),
} };
test('table arrows scroll the named region and a long descriptor expands in the page at 400px, in both themes and languages', async ({ page }) => {
  await scenario();
  await page.route('**/api/v1/harness-kinds', (route) => route.fulfill({ json: [adapter] }));
  await signInForManagement(page);
  for (const locale of ['en', 'zh-CN'] as const) {
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const chinese = locale === 'zh-CN';
    await page.goto('/admin/harnesses');
    for (const theme of ['light', 'dark'] as const) {
      await page.getByRole('button', { name: chinese ? '你的账号' : 'Your account', exact: true }).click();
      await page.getByRole('menuitemradio', { name: chinese ? theme === 'light' ? '浅色' : '深色' : theme === 'light' ? 'Light' : 'Dark', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await page.setViewportSize({ width: 400, height: 860 });
      const table = page.getByRole('region', { name: chinese ? '各适配器支持的功能' : 'What each adapter supports', exact: true });
      await table.focus();
      await expect(table).toBeFocused();
      await page.keyboard.press('Home');
      await page.keyboard.press('ArrowRight');
      await expect.poll(() => table.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);
      const label = chinese ? 'Research adapter 的描述' : 'The descriptor of Research adapter';
      const descriptor = page.locator('details').filter({ has: page.locator('summary').filter({ hasText: label }) });
      if (!(await descriptor.evaluate((element) => element instanceof HTMLDetailsElement && element.open))) await descriptor.locator('summary').click();
      const less = descriptor.getByRole('button', { name: chinese ? '收起' : 'Show less', exact: true });
      if (await less.isVisible()) await less.click();
      await expect(descriptor.getByText('field11:', { exact: true })).toHaveCount(0);
      await descriptor.getByRole('button', { name: chinese ? '显示全部' : 'Show all', exact: true }).click();
      await expect(descriptor.getByText('field11:', { exact: true })).toBeVisible();
      await descriptor.getByRole('button', { name: chinese ? `收起 ${label}` : `Collapse ${label}`, exact: true }).focus();
      await page.keyboard.press('Space');
      const toggle = descriptor.getByRole('button', { name: chinese ? `展开 ${label}` : `Expand ${label}`, exact: true });
      await expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await expect(toggle).toBeFocused();
      const target = await toggle.boundingBox();
      expect(target?.width).toBeGreaterThanOrEqual(24);
      expect(target?.height).toBeGreaterThanOrEqual(24);
      expect(await descriptor.evaluate((element) => Array.from(element.querySelectorAll('*')).every((part) => {
        const overflow = getComputedStyle(part).overflowY;
        return !['auto', 'scroll'].includes(overflow) || part.scrollHeight <= part.clientHeight;
      }))).toBe(true);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
  }
});
