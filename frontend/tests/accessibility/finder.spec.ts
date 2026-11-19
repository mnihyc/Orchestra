/** Axe over the finder, the search, the find bar and the help, in both themes: the finder empty, with results of every group and as the card of a person, the search page with its filters and a picker open, a hint's card, and the help view and page. */
import { expect, type Page, test } from '@playwright/test';

import { userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, emit, expectAccessibleInBothThemes, playScene, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
  await playScene(ACME.mainThread, 'history');
});

const finder = (page: Page) => page.getByRole('dialog', { name: 'Find', exact: true });
const field = (page: Page) => finder(page).getByRole('combobox', { name: 'Search terms', exact: true });

const atRest = (page: Page): Promise<void> =>
  page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestIdleCallback(() => resolve(), { timeout: 2_000 }))));

async function openFinder(page: Page): Promise<void> {
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('Control+k');
  await expect(field(page)).toBeFocused();
}

test('the finder, empty and with results of every group, in both themes', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login', level: 1 })).toBeVisible();
  await openFinder(page);
  await expect(finder(page).getByRole('option').first()).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await field(page).pressSequentially('idle', { delay: 20 });
  await expect(finder(page).getByRole('option', { name: /Idle stop/ })).toBeVisible();
  await expect(finder(page).getByRole('option', { name: /Idle-stop rework/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await field(page).fill('>new');
  await expect(finder(page).getByRole('option', { name: /New thread/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await field(page).fill('zzzqqq');
  await expect(finder(page).getByText(/Nothing found/).first()).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the card of a person found by name, in both themes', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await field(page).pressSequentially('@dora', { delay: 20 });
  await finder(page).getByRole('option', { name: /Dora Developer/ }).click();
  await expect(finder(page).getByRole('region', { name: 'Dora Developer' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the search page with its results, Filters and a nested picker, in both themes', async ({ page }) => {
  await signIn(page);
  await emitMore();
  await page.goto('/search?q=redirect');
  await expect(page.getByRole('region', { name: 'Results' }).getByRole('link').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Filters', exact: true }).click();
  await expectAccessibleInBothThemes(page, '[aria-label="Narrow the search"][role="dialog"]');
  await page.getByRole('button', { name: /Where: Everything you can read/ }).click();
  await page.getByPlaceholder('Find a place by name').fill('fix');
  await expect(page.getByRole('button', { name: /Fix login/ }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page, '[aria-label="Where"][role="dialog"]');
  await page.keyboard.press('Escape');

  await expect(page.getByRole('dialog', { name: 'Narrow the search' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Filters', exact: true })).toBeFocused();
});

test('the search page when it has nothing to show, in both themes', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByText('Search what was said')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto('/search?q=zzzzzz');
  await expect(page.getByText('Nothing found for “zzzzzz”')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the bar that finds in a conversation, in both themes', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Find in the conversation', exact: true }).click();
  const bar = page.getByRole('search', { name: 'Find in the conversation' });
  await bar.getByRole('textbox').fill('redirect');
  await expect(bar.getByRole('status')).toHaveText(/^1 of/);
  await expectAccessibleInBothThemes(page, '[role="search"]');
});

test('the help beside a screen, its tabs, an article, and the help as a page, in both themes', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByRole('heading', { name: 'Search', level: 1 })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('F1');
  const view = page.getByRole('region', { name: 'Right bar' }).getByRole('region', { name: 'Help' });
  await expect(view.getByRole('heading', { name: 'What this screen is for' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[aria-label="Help"]');

  await view.getByRole('tab', { name: 'Keys' }).click();
  await expectAccessibleInBothThemes(page, '[aria-label="Help"]');
  await view.getByRole('tab', { name: /Articles/ }).click();
  await view.getByRole('button', { name: /Finding things/ }).click();
  await expect(view.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[aria-label="Help"]');

  await page.goto('/help/finding-things');
  await expect(page.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

/** A message for the search to find besides the scene's own. */
async function emitMore(): Promise<void> {
  await emit(ACME.mainThread, { items: [userMessage('redirect-more', 'More about the redirect that was asked for.', { author: ADA, at: '2026-10-02T10:00:00.000Z' })] });
}
