/** Hints and help at work: a term's card, the docked F1 view and standalone guide, article anchors, and the finder's explicit controls. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const rightBar = (page: Page) => page.getByRole('region', { name: 'Right bar' });
const helpView = (page: Page) => rightBar(page).getByRole('region', { name: 'Help' });

/** Waits until the page is at rest: the keys of the workbench are listened for once its frame is drawn. */
const atRest = (page: Page): Promise<void> =>
  page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestIdleCallback(() => resolve(), { timeout: 2_000 }))));

test('a search hint explains its combined results and leads to the matching article heading', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await page.getByRole('button', { name: 'Help: Search results' }).click();

  const card = page.getByRole('dialog', { name: 'Search results' });
  await expect(card).toBeVisible();
  await expect(card.getByText('What this is')).toBeVisible();
  await expect(card.getByText('Words and similar meaning arrive in one list; Show more continues it.')).toBeVisible();
  // The unified search hint intentionally has only What; optional choice advice remains covered by HelpHint's component cases.
  await expect(card.getByText('Which one for me')).toHaveCount(0);

  // A hint's prompt may use the temporary reading view; F1's docked behavior is checked separately below.
  await card.getByRole('button', { name: 'Read more' }).click();
  await expect(card).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
  await expect(page.locator('[id="words-and-similar-meaning"]')).toBeInViewport();
});

test('related search controls share one guide question and their mounted explanations stay in This screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.locator('[data-help-article="finding-things"][data-help-heading="words-and-similar-meaning"]')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Help: Dates' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Help: Search results' }).click();
  const card = page.getByRole('dialog', { name: 'Search results' });
  await expect(card.getByText('What this is')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await atRest(page);
  await page.keyboard.press('F1');
  // This screen lists the questions actually mounted; File type is the current distinct filter hint.
  await expect(helpView(page).getByRole('region', { name: 'Hints on this screen' })).toContainText('File type');
  await expect(helpView(page).getByRole('region', { name: 'Hints on this screen' })).toContainText('Some conversation files have no recorded type. Choose Any to include them.');
});

test('F1 opens the help of the screen in front, beside it: what it is for, its hints, the articles that belong to it', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByRole('heading', { name: 'Search results', level: 1 })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('F1');

  await expect(page.locator('[data-workbench-bar="right"]')).toHaveAttribute('data-active-container', 'help');
  const view = helpView(page);
  await expect(view.getByRole('heading', { name: 'What this screen is for' })).toBeVisible();
  // The hints that are on this screen are listed with what they say.
  await expect(view.getByRole('region', { name: 'Hints on this screen' })).toContainText('Search results');
  // And the article whose front matter names this route.
  await view.getByRole('button', { name: 'Finding things' }).click();
  await expect(view.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
  await expect(view.getByRole('heading', { name: 'The finder' })).toBeVisible();
  await view.getByRole('button', { name: 'All articles' }).click();
  await expect(view.getByRole('tab', { name: /^Articles/ })).toHaveAttribute('aria-selected', 'true');
});

test('the help is searched by title and heading, and the keys listed are the ones bound', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByRole('heading', { name: 'Search results', level: 1 })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('F1');
  const view = helpView(page);
  await view.getByRole('searchbox', { name: 'Search the help' }).fill('conversation');
  await view.getByRole('button', { name: /Find in a conversation/ }).click();
  await expect(view.getByRole('heading', { name: 'Find in a conversation' })).toBeInViewport();

  await view.getByRole('button', { name: 'All articles' }).click();
  await view.getByRole('tab', { name: 'Keys' }).click();
  await expect(view.getByText('Find anything, or run a command')).toBeVisible();
  await expect(view.getByRole('tab', { name: 'Tutorials' })).toBeVisible();
});

test('the help is a page of its own, and an article has an address with an anchor', async ({ page }) => {
  await signIn(page);
  await page.goto('/help');
  await expect(page.getByRole('searchbox', { name: 'Search the help' })).toBeVisible();
  await page.getByRole('tab', { name: /Articles/ }).click();
  await page.getByRole('button', { name: /Finding things/ }).click();
  await expect(page).toHaveURL(/\/help\/finding-things$/);
  await expect(page.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();

  await page.goto('/help/finding-things#find-in-a-conversation');
  await expect(page.getByRole('heading', { name: 'Find in a conversation' })).toBeInViewport();
  // A link to another article stays in the help, and an address that is no article says so.
  await page.goto('/help/nothing-here');
  await expect(page.getByText('There is no article at this address')).toBeVisible();
});

test('the finder keeps explicit mode controls after reload and resetting closed tips without an unsolicited note', async ({ page }) => {
  await signIn(page);
  await page.goto(`/org/acme`);
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('Control+k');
  const tip = page.getByRole('note').filter({ hasText: 'Type > for commands' });
  // The finder deliberately retired this first-use tip; its explicit mode controls remain the way on.
  await expect(tip).toHaveCount(0);
  await expect(page.getByRole('dialog', { name: 'Find' }).getByRole('group', { name: 'Keep the finder to' }).getByRole('button', { name: 'Commands', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  // Reload and the generic tip reset do not restore a retired note.
  await page.reload();
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: 'Find' })).toBeVisible();
  await expect(page.getByRole('note').filter({ hasText: 'Type > for commands' })).toHaveCount(0);
  await page.keyboard.press('Escape');

  await page.keyboard.press('F1');
  await helpView(page).getByRole('button', { name: 'Show closed tips again' }).click();
  await page.keyboard.press('Control+k');
  await expect(tip).toHaveCount(0);
  await expect(page.getByRole('dialog', { name: 'Find' }).getByRole('group', { name: 'Keep the finder to' }).getByRole('button', { name: 'Commands', exact: true })).toBeVisible();
});

test('on every screen that has a route, F1 opens the help with something for it', async ({ page }) => {
  await signIn(page);
  for (const path of ['/inbox', '/approvals', '/resources/providers', '/settings/profile', `/thread/${ACME.mainThread}`, `/topic/${ACME.fixLogin}`, '/org/acme']) {
    await page.goto(path);
    await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
    await atRest(page);
    await page.keyboard.press('F1');
    await expect(helpView(page).getByRole('heading', { name: 'What this screen is for' }), path).toBeVisible();
    // Each says what the screen is for in words, and not that it knows nothing of it.
    await expect(helpView(page).getByText('Nothing is written about this screen yet.'), path).toHaveCount(0);
  }
});
