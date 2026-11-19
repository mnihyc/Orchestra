/** Where the left bar meets the rest of the application: the colour a place was given in the bar is the colour of its icon wherever it is drawn, an org has one colour in the rail, on its home and in its settings, an org reached by its id is taken to its address with what the address said, and the chat of an org has one name. */
import { expect, type Page, test } from '@playwright/test';

import { composer } from './chat-support.ts';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const main = (page: Page) => page.getByTestId('main');

/** The colour dialog of a place, opened from its "⋯" in the left bar, and one of the twelve chosen in it by name. */
async function colour(page: Page, place: string, name: string): Promise<void> {
  await bar(page).getByRole('button', { name: new RegExp(`^Actions for ${place}(?: · .+)?$`), exact: true }).first().click();
  await page.getByRole('menuitem', { name: 'Colour…' }).click();
  const dialog = page.getByRole('dialog', { name: `Colour of ${place}` });
  await dialog.getByRole('button', { name }).click();
  await expect(dialog.getByRole('button', { name })).toHaveAttribute('aria-pressed', 'true');
  await dialog.getByRole('button', { name: 'Close' }).first().click();
  await expect(dialog).toHaveCount(0);
}

test('a place has the icon of its kind in the colour it was given wherever it is drawn: its card, its own page, the finder and the dialog that exports it', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(bar(page).getByRole('treeitem', { name: /^Shop/ })).toBeVisible();
  // As shipped a project is blue, the colour of its kind, wherever it stands.
  await expect(main(page).locator('article', { hasText: 'Shop' }).locator('[data-node-icon="project"]')).toHaveAttribute('data-node-color', '8');

  await colour(page, 'Shop', 'Green');
  // Its card on the home of the org.
  await expect(main(page).locator('article', { hasText: 'Shop' }).locator('[data-node-icon="project"]')).toHaveAttribute('data-node-color', '5');

  // Its own page, in the mark at the head of it.
  await page.goto(`/project/${ACME.shop}`);
  await expect(main(page).getByRole('heading', { name: 'Shop', level: 1 })).toBeVisible();
  await expect(main(page).locator('[data-node-icon="project"]').first()).toHaveAttribute('data-node-color', '5');

  // The finder, which draws a project as the bar does and not as a folder.
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await page.keyboard.press('Control+k');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await expect(finder).toBeVisible();
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).pressSequentially('Shop', { delay: 20 });
  const found = finder.getByRole('option', { name: /^Shop/ }).first();
  await expect(found.locator('[data-node-icon="project"]')).toHaveAttribute('data-node-color', '5');
  await page.keyboard.press('Escape');

  // The tree of ticks in the dialog that exports the org.
  await page.goto('/org/acme');
  await bar(page).getByRole('button', { name: /^Actions for Acme(?: · .+)?$/, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  const exporting = page.getByRole('dialog', { name: 'Export Acme' });
  await expect(exporting.getByRole('treeitem', { name: /^Shop/ }).first().locator('[data-node-icon="project"]').first()).toHaveAttribute('data-node-color', '5');
});

test('an org has one colour: the one it was given in the bar is the mark of its stop in the rail, of its home and of its settings', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(bar(page).getByRole('treeitem', { name: /^Web/ })).toBeVisible();
  await colour(page, 'Acme', 'Green');
  const mark = (where: Page) => where.locator('nav[aria-label="Orgs and your own places"] a[href="/org/acme"] .avatar');
  await expect(mark(page)).toHaveClass(/bg-\(--identity-4\)/);
  // Its home.
  await expect(main(page).locator('.avatar').first()).toHaveClass(/bg-\(--identity-4\)/);
  // Its settings.
  await page.goto('/org/acme/settings/general');
  await expect(page.getByRole('heading', { name: 'Settings of Acme', level: 1 })).toBeVisible();
  await expect(page.locator('h1', { hasText: 'Settings of Acme' }).locator('xpath=ancestor::*[.//*[contains(@class, "avatar")]][1]').locator('.avatar').first()).toHaveClass(/bg-\(--identity-4\)/);
});

test('an org reached by its id is taken to its address with what the address said: the chat panel opens, and the chat of an org is called Team chat in the bar and in the panel', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/org/${ACME.org}?panel=chat`);
  await expect(page).toHaveURL(/\/org\/acme\?panel=chat$/);
  await expect(composer(page)).toBeVisible();
  // One word for it: the bar's row, and the panel's header.
  await expect(bar(page).getByRole('link', { name: 'Team chat' })).toBeVisible();
  await expect(page.getByText('Team chat', { exact: true }).first()).toBeVisible();
  await expect(page.getByText(/org chat/i)).toHaveCount(0);
});
