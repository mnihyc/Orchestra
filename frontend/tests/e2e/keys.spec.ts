/** A person's own keys: single keys switched off and on again, an action given the keys pressed for it, and keys another action has refused with its name. */
import { expect, test } from '@playwright/test';

import { scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('single keys switched off stop acting, keys with a modifier still act, and the list of keys stays within reach', async ({ page }) => {
  await signIn(page);
  await page.goto('/settings/interface');
  const single = page.getByRole('switch', { name: 'Single-key shortcuts' });
  await expect(single).toBeChecked();
  await single.click();
  await expect(single).not.toBeChecked();
  // Saved: the switch is free again, and what is kept acts.
  await expect(single).toBeEnabled();

  await page.locator('body').press('g');
  await page.locator('body').press('i');
  await expect(page).toHaveURL(/\/settings\/interface$/);
  await page.locator('body').press('?');
  await expect(page.getByRole('dialog', { name: 'Keyboard shortcuts' })).toHaveCount(0);
  await page.locator('body').press('Control+k');
  await expect(page.getByRole('dialog', { name: 'Find', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  // The list is reached from the page, and says which keys are off.
  await page.getByRole('button', { name: 'Change keys' }).click();
  const list = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  await expect(list.locator('[data-key-action="inbox"]')).toContainText('Off');
  await expect(list.locator('[data-key-action="inbox"]')).toHaveAttribute('aria-description', 'Off: single keys are switched off');
  await expect(list.locator('[data-key-action="finder"]')).not.toContainText('Off');
  await page.keyboard.press('Escape');

  await single.click();
  await expect(single).toBeChecked();
  await expect(single).toBeEnabled();
  await page.locator('body').press('g');
  await page.locator('body').press('i');
  await expect(page).toHaveURL(/\/inbox$/);
});

test('an action takes the keys pressed for it, and keys another action has are refused with its name', async ({ page }) => {
  await signIn(page);
  await page.locator('body').press('?');
  const list = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  const inbox = list.locator('[data-key-action="inbox"]');

  await inbox.getByRole('button', { name: 'Change the keys of Go to the inbox' }).click();
  await expect(list.getByRole('group', { name: 'New keys for Go to the inbox' })).toBeFocused();
  await page.keyboard.press('Control+k');
  await expect(inbox.getByRole('alert')).toHaveText('Mod+K already does something else: Find anything, or run a command.');

  await inbox.getByRole('button', { name: 'Change the keys of Go to the inbox' }).click();
  await page.keyboard.press('g');
  await page.keyboard.press('n');
  await expect(inbox.getByRole('button', { name: 'Give Go to the inbox back G I' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(list).toHaveCount(0);

  // The new keys act, and the old ones no longer do.
  await page.locator('body').press('g');
  await page.locator('body').press('i');
  await expect(page).not.toHaveURL(/\/inbox$/);
  await page.locator('body').press('g');
  await page.locator('body').press('n');
  await expect(page).toHaveURL(/\/inbox$/);

  // Reset gives the action back the keys it shipped with.
  await page.locator('body').press('?');
  await inbox.getByRole('button', { name: 'Give Go to the inbox back G I' }).click();
  await expect(inbox.getByRole('button', { name: 'Give Go to the inbox back G I' })).toHaveCount(0);
});
