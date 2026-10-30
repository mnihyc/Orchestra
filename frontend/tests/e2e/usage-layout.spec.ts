/** Usage follows the same control sizes and responsive tables as the resource lists. */
import { expect, test } from '@playwright/test';
import { ACME, expectAccessible, scenario, signIn } from './support';
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test.beforeEach(async ({ request }) => {
  await scenario();
  expect((await request.post(`${DOUBLE}/__double/usage/performance`)).ok()).toBe(true);
});

for (const width of [400, 768, 1100, 1440]) {
  test(`all Usage tabs fit their page at ${width}px in both themes`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await signIn(page);
    for (const theme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme: theme });
      await page.goto('/resources/providers');
      const ordinary = page.locator('[data-resource-navigation]').getByRole('link', { name: 'Providers', exact: true });
      await expect(ordinary).toBeVisible();
      const ordinaryHeight = await ordinary.evaluate((node) => node.getBoundingClientRect().height);
      await page.goto('/usage?range=7d');
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      const tabs = page.getByRole('tablist', { name: 'Usage views', exact: true });
      for (const name of ['Consumption', 'Performance', 'Requests']) {
        const selected = tabs.getByRole('tab', { name, exact: true });
        await selected.click();
        const panelId = await selected.getAttribute('aria-controls');
        expect(panelId).toBeTruthy();
        const panel = page.locator(`[id="${panelId}"]`);
        await expect(panel).toHaveAttribute('role', 'tabpanel');
        await expect(panel).toHaveAccessibleName(name);
        await expect(panel).toBeVisible();
        for (const value of ['Consumption', 'Performance', 'Requests']) {
          const tab = tabs.getByRole('tab', { name: value, exact: true });
          await expect(tab).toBeVisible();
          // One CSS pixel allows subpixel rounding; descriptions must not enlarge a tab.
          expect(Math.abs(await tab.evaluate((node) => node.getBoundingClientRect().height) - ordinaryHeight)).toBeLessThanOrEqual(1);
        }
        await expect(page).toHaveURL(/range=7d/);
        await expect(page.getByRole('radio', { name: '7 days', exact: true })).toHaveAttribute('aria-checked', 'true');
        await expect(page.getByText('Days use UTC',{exact:true})).toHaveCount(1);
        await expect(page.getByRole('button', { name: 'Speed, first output and errors', exact: true })).toHaveCount(1);
        await expect(page.getByText(/^Seen by /)).toHaveCount(0);
        if (name === 'Performance') await expect(panel.getByRole('table', { name: 'Compare models', exact: true })).toContainText('fast-model');
        if (name === 'Requests') await expect(panel.getByRole('table', { name: 'Requests', exact: true })).toContainText('fast-model');
        const extent = await panel.evaluate((node) => {
          const frame = node.getBoundingClientRect();
          return Array.from(node.querySelectorAll('table'), (table) => ({ left: table.getBoundingClientRect().left - frame.left, right: table.getBoundingClientRect().right - frame.right }));
        });
        for (const table of extent) { expect(table.left).toBeGreaterThanOrEqual(-1); expect(table.right).toBeLessThanOrEqual(1); }
      }
      // The independent reviews cover every route; retain focused phone/desktop a11y proofs here.
      if (width === 400 || width === 1440) {
        const labelledBy = await tabs.getByRole('tab', { name: 'Requests', exact: true }).getAttribute('id');
        await expectAccessible(page, theme, `[role="tabpanel"][aria-labelledby="${labelledBy}"]`);
      }
      const image = test.info().outputPath(`usage-requests-${width}-${theme}.png`);
      await page.screenshot({ path: image, fullPage: true, animations: 'disabled' });
      await test.info().attach(`usage-requests-${width}-${theme}`, { path: image, contentType: 'image/png' });
    }
  });
}

test('the place access page is one click away without an audience sentence', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${ACME.web}&range=7d`);
  await expect(page.getByText(/^Seen by /)).toHaveCount(0);
  await page.getByRole('button', { name: 'Access to Web', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveAccessibleName(/Web/);
});
