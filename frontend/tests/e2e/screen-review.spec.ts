/** Screen review regressions: personal frames, contextual settings, topic wording, mobile paths and visible keyboard focus. */
import { expect, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('notifications share the personal frame and legacy usage leads to the one usage page', async ({ page }) => {
  await signIn(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/settings/notifications');
  const navigation = page.getByRole('navigation', { name: 'Your settings', exact: true });
  // The personal frame uses its section picker when the content area is narrow.
  const sections = navigation.getByRole('combobox', { name: 'Your settings', exact: true });
  await expect(sections).toHaveValue('Notifications');
  await sections.click();
  await page.getByRole('option', { name: 'Security', exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/security$/);
  await sections.click();
  await page.getByRole('option', { name: 'Usage and limits', exact: true }).click();
  await expect(page).toHaveURL(/\/usage(?:\?|$)/);
  await page.goto('/settings/usage');
  await expect(page).toHaveURL(/\/usage(?:\?|$)/);
  await expect(page.getByRole('heading', { name: 'Usage', level: 1, exact: true })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Scope', exact: true })).toHaveValue('Me');
});

test('a project starts a topic and its settings tab keeps the section and place', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('button', { name: 'New topic', exact: true })).toBeVisible();
  await page.goto(`/project/${ACME.web}/settings`);
  await expect(page).toHaveTitle(/General · Web/);
  await page.goto('/org/acme/settings/defaults');
  await expect(page).toHaveTitle(/Defaults · Acme/);
  await expect(page.locator('a[aria-current="page"]').filter({ hasText: /^Defaults$/ })).toHaveCount(1);
});

test('a phone keeps a readable topic title and its nearest parent back link', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  const path = page.getByRole('navigation', { name: 'Where this is', exact: true });
  const current = page.getByRole('heading', { name: 'Fix login', level: 1, exact: true });
  await expect(current).toContainText('Fix login');
  await expect.poll(() => current.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  // Parents are no longer put into an overflow menu: phones retain the nearest parent as their back link.
  await expect(path.locator('a:visible')).toHaveCount(1);
  const parent = path.getByRole('link', { name: 'Web', exact: true });
  await expect(parent).toBeVisible();
  await expect(path.getByRole('button')).toHaveCount(0);
  await parent.click();
  await expect(page.getByRole('heading', { name: 'Web', level: 1, exact: true })).toBeVisible();
});

test('an org child link shows a keyboard outline inside its own bounds', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  // The org home uses its native child list; focus belongs to the link itself rather than a card pseudo-element.
  const link = page.getByRole('main', { name: 'Main area', exact: true }).getByRole('link', { name: 'Infra', exact: true });
  await expect(link).toBeVisible();
  // Keyboard modality matters: an ordinary pointer focus need not draw focus-visible.
  await page.keyboard.press('Tab');
  await link.focus();
  await expect.poll(() => link.evaluate((element) => {
    const style = getComputedStyle(element);
    return style.outlineStyle !== 'none' && Number.parseFloat(style.outlineWidth) >= 2 && Number.parseFloat(style.outlineOffset) < 0;
  })).toBe(true);
});
