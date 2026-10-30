/** Help and the guides of how things work (approvals, ports, an environment changed while it runs, who can see and do what), asked in both themes whether anything on them is a serious barrier: the Help page in each of its tabs, each guide as a page, on a phone and in Chinese, and a tutorial in its player, left by keyboard alone. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

const GUIDES = [
  { slug: 'how-approvals-work', title: 'How approvals work' },
  { slug: 'let-agents-open-ports', title: 'Let agents open ports without asking' },
  { slug: 'change-a-running-environment', title: 'Change an environment while it runs' },
  { slug: 'who-can-see-and-do-what', title: 'Who can see and do what' },
] as const;

async function inChinese(page: Page): Promise<void> {
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
}

test('the Help page in each of its tabs', async ({ page }) => {
  await signIn(page);
  await page.goto('/help');
  const tabs = page.getByRole('tablist').filter({ has: page.getByRole('tab', { name: /^Tutorials/ }) });
  await expect(tabs).toBeVisible();
  const count = await tabs.getByRole('tab').count();
  expect(count).toBeGreaterThan(3);
  for (let index = 0; index < count; index += 1) {
    const tab = tabs.getByRole('tab').nth(index);
    await tab.click();
    await expect(tab).toHaveAttribute('aria-selected', 'true');
    await expectAccessibleInBothThemes(page);
  }
});

test('each guide of how things work, as a page, then on a phone', async ({ page }) => {
  await signIn(page);
  for (const guide of GUIDES) {
    await page.goto(`/help/${guide.slug}`);
    await expect(page.getByRole('heading', { name: guide.title }).first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
  await page.setViewportSize({ width: 390, height: 844 });
  for (const guide of GUIDES) {
    await page.goto(`/help/${guide.slug}`);
    await expect(page.getByRole('heading', { name: guide.title }).first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
});

test('each guide of how things work in Chinese', async ({ page }) => {
  await signIn(page);
  await inChinese(page);
  for (const guide of GUIDES) {
    await page.goto(`/help/${guide.slug}`);
    await expect(page.getByRole('heading').first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
});

test('a tutorial in its player: what it asks, and the way out by keyboard alone', async ({ page }) => {
  await signIn(page);
  await page.goto('/help');
  await page.getByRole('tab', { name: /^Tutorials/ }).click();
  const card = page.locator('section, article, li, div').filter({ hasText: GUIDES[0].title }).filter({ has: page.getByRole('button', { name: 'Start tutorial' }) }).last();
  await card.getByRole('button', { name: 'Start tutorial' }).click();
  const player = page.getByRole('region', { name: 'Tutorial' });
  await expect(player).toBeVisible();
  await expect(player.getByRole('status').first()).toHaveText(/Step 1 of \d+/);
  await expectAccessibleInBothThemes(page);

  // The player is reached by the keyboard and left with it; what was in front stays.
  await player.getByRole('button', { name: 'Leave tutorial' }).focus();
  await expect(player.getByRole('button', { name: 'Leave tutorial' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(player).toHaveCount(0);
  await expect(page).toHaveURL(/\/resources\/environments/);
});
