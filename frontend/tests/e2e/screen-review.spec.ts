/** Screen review regressions: personal frames, contextual settings, topic wording, mobile paths and visible keyboard focus. */
import { expect, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('notifications share the personal frame and legacy usage leads to the one usage page', async ({ page }) => {
  await signIn(page);
  await page.goto('/settings/notifications');
  const navigation = page.getByRole('navigation', { name: 'Your settings', exact: true });
  await expect(navigation.getByRole('link', { name: 'Notifications', exact: true })).toHaveAttribute('aria-current', 'page');
  await expect(navigation.getByRole('link', { name: 'Security', exact: true })).toHaveAttribute('href', '/settings/security');
  await expect(navigation.getByRole('link', { name: 'Usage and limits', exact: true })).toHaveAttribute('href', '/usage');
  await page.goto('/settings/usage');
  await expect(page).toHaveURL(/\/usage(?:\?|$)/);
  await expect(page.getByRole('heading', { name: 'Your usage', level: 1 })).toBeVisible();
});

test('a project starts a topic and its settings tab keeps the section and place', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('button', { name: 'Start topic', exact: true })).toBeVisible();
  await page.goto(`/project/${ACME.web}/settings`);
  await expect(page).toHaveTitle(/General · Web/);
  await page.goto('/org/acme/settings/policies');
  await expect(page).toHaveTitle(/Defaults · Acme/);
  await expect(page.locator('a[aria-current="page"]').filter({ hasText: /^Defaults$/ })).toHaveCount(1);
});

test('a phone folds the topic ancestors while retaining a readable current title and the complete omitted path', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  const path = page.getByRole('navigation', { name: 'Where this is', exact: true });
  const current = path.locator('[aria-current="page"]').last();
  await expect(current).toContainText('Fix login');
  await expect.poll(() => current.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  const folded = path.getByRole('button');
  await expect(folded).toHaveCount(1);
  await folded.click();
  await expect(page.getByRole('menu')).toContainText('Web');
});

test('an org card shows a keyboard outline inside its own bounds', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const link = page.locator('article a').filter({ hasText: 'Web' }).first();
  await expect(link).toBeVisible();
  // Keyboard modality matters: an ordinary pointer focus need not draw focus-visible.
  await page.keyboard.press('Tab');
  await link.focus();
  await expect.poll(() => link.evaluate((element) => {
    const style = getComputedStyle(element, '::after');
    return style.outlineStyle !== 'none' && Number.parseFloat(style.outlineWidth) >= 2 && Number.parseFloat(style.outlineOffset) < 0;
  })).toBe(true);
});
