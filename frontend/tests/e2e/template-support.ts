/** Template configuration uses the same section links at desktop and phone widths: a list beside the settings, a strip above them. */
import { expect, type Page } from '@playwright/test';

export async function templateSection(page: Page, name: string): Promise<void> {
  const nav = page.locator('#template-spec [data-section-nav]');
  await expect(nav).toBeVisible();
  // A list beside the settings or a strip above them: the same names, never a dropdown.
  await nav.getByRole('link', { name, exact: true }).click();
}

export async function templateJson(page: Page): Promise<void> {
  const toggle = page.getByRole('switch', { name: 'JSON', exact: true });
  if (!await toggle.isVisible()) await page.locator('#template-spec [data-template-settings] [data-resource-details]').first().locator('summary').click();
  await toggle.click();
}
