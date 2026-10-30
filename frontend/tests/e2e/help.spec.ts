/** Hints and help at work: the "?" beside a term with its card, the help view beside the screen in front (`F1`) and as a page of its own, the articles of the guide with their anchors, and the tip of the finder that is closed for good. */
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

test('a hint is a "?" beside a term, and its card says what it is, which one to take, and leads to the article', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await page.getByRole('button', { name: 'Help: Where to search' }).click();

  const card = page.getByRole('dialog', { name: 'Where to search' });
  await expect(card).toBeVisible();
  await expect(card.getByText('What this is')).toBeVisible();
  await expect(card.getByText(/looks through everything you may read/)).toBeVisible();
  await expect(card.getByText('Which one for me')).toBeVisible();
  await expect(card.getByText(/two characters/)).toBeVisible();

  // More: the article, beside what the person is doing, at the heading that treats it.
  await card.getByRole('button', { name: 'Read more' }).click();
  await expect(card).toHaveCount(0);
  await expect(helpView(page).getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
  await expect(helpView(page).locator('[id="search"]')).toBeInViewport();
});

test('related search controls share one guide question and their distinct explanations stay in This screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.locator('[data-help-article="finding-things"][data-help-heading="search"]')).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Help: Dates' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Help: Where to search' }).click();
  const card = page.getByRole('dialog', { name: 'Where to search' });
  await expect(card.getByText('What this is')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await atRest(page);
  await page.keyboard.press('F1');
  await expect(helpView(page).getByText('Dates', { exact: true })).toBeVisible();
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
  await expect(view.getByText('Where to search', { exact: true })).toBeVisible();
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
  await expect(page.getByRole('region', { name: 'Help' })).toBeVisible();
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

test('a tip is shown unasked, closed for good, and brought back from the help', async ({ page }) => {
  await signIn(page);
  await page.goto(`/org/acme`);
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('Control+k');
  const tip = page.getByRole('note').filter({ hasText: 'Type > for commands' });
  await expect(tip).toBeVisible();
  await tip.getByRole('button', { name: 'Got it, do not show this again' }).click();
  await expect(tip).toHaveCount(0);
  await page.keyboard.press('Escape');

  // It stays closed, also after the page is loaded anew.
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
  await expect(page.getByRole('note').filter({ hasText: 'Type > for commands' })).toBeVisible();
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
