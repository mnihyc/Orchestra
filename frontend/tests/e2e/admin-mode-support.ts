/** The sole visible session switch is used explicitly by scenarios which need administrator reach. */
import { expect, type Page } from '@playwright/test';
import { PEOPLE, signIn } from './support.ts';

function adminSwitch(page: Page) {
  return page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: /^As admin: (Off|On)$/ });
}

export async function asAdmin(page: Page, enabled: boolean): Promise<void> {
  const indicator = adminSwitch(page);
  await expect(indicator).toBeVisible();
  if (await indicator.getAttribute('data-as-admin') === String(enabled)) return;
  await indicator.click();
  await expect(indicator).toHaveAttribute('data-as-admin', String(enabled));
  await expect(indicator).toHaveAttribute('aria-checked', String(enabled));
}

export async function signInForManagement(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  // Once the title bar is ready, its controls reflect the signed-in person's reach.
  await expect(page.locator('[data-screen-titlebar]:visible')).toHaveAttribute('aria-busy', 'false');
  if (await adminSwitch(page).isVisible()) await asAdmin(page, true);
}
