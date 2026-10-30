/** Template configuration uses the same section navigation at desktop and phone widths. */
import { expect, type Page } from '@playwright/test';

export async function templateSection(page: Page, name: string): Promise<void> {
  const nav = page.locator('#template-spec [data-section-nav]');
  await expect(nav).toBeVisible();
  const chooser = nav.getByRole('combobox');
  if (await chooser.isVisible()) {
    await chooser.click();
    await page.getByRole('option', { name, exact: true }).click();
  } else await nav.getByRole('link', { name, exact: true }).click();
}

export async function templateJson(page: Page): Promise<void> {
  const toggle = page.getByRole('switch', { name: 'JSON', exact: true });
  if (!await toggle.isVisible()) await page.locator('#template-spec [data-template-settings] [data-resource-details]').first().locator('summary').click();
  await toggle.click();
}
