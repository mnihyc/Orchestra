/** Choose the audit language through the person's Interface page. */
import { expect, type Page } from '@playwright/test';

export async function auditLanguage(page: Page, language: 'en' | 'zh-CN'): Promise<void> {
  if (language === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: 'Language', exact: true }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', language);
}
