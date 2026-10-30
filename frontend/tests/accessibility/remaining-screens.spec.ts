/** The screens no other accessibility check asks, in both themes: what was shared with a person, Approvals in each of its tabs, a search that finds nothing on a phone, the lists of MCP servers and mounts, one's own usage and limits, the inbox in Chinese on a phone, the refusal of Management for a reader, and an address that leads nowhere on a phone. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';
import { RUNS_ON } from '../doubles/fixtures';
import { expectUniqueGuideEntries } from './guide-entries';

test.beforeEach(async () => {
  await scenario();
});

const PHONE = { width: 390, height: 844 };

async function inChinese(page: Page): Promise<void> {
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
}

test('what was shared with me, and Approvals in each of its tabs', async ({ page }) => {
  await signIn(page);
  await page.goto('/shared');
  await expect(page.getByRole('heading', { name: 'Shared with me' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto('/approvals');
  const tabs = page.getByRole('tablist').filter({ has: page.getByRole('tab', { name: 'History' }) });
  await expect(tabs).toBeVisible();
  for (const name of ['Waiting for me', 'Requested by me', 'History']) {
    await tabs.getByRole('tab', { name }).click();
    await expect(tabs.getByRole('tab', { name })).toHaveAttribute('aria-selected', 'true');
    await expectAccessibleInBothThemes(page);
  }
});

test('a search that finds nothing, on a phone', async ({ page }) => {
  await page.setViewportSize(PHONE);
  await signIn(page);
  await page.goto('/search?q=zzzqqq-nothing');
  await expect(page.getByText(/Nothing found for/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the lists of MCP servers and of mounts, and one’s own usage and limits', async ({ page }) => {
  await signIn(page);
  for (const path of ['/resources/mcp', '/resources/mounts']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
  await page.goto('/settings/usage');
  await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the inbox in Chinese on a phone', async ({ page }) => {
  await signIn(page);
  await inChinese(page);
  await page.setViewportSize(PHONE);
  await page.goto('/inbox');
  await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('Management refused to a reader, and an address that leads nowhere on a phone', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto('/admin/overview');
  await expect(page.getByText('Management is for system administrators')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.setViewportSize(PHONE);
  await page.goto('/nowhere/at/all');
  await expect(page.getByText('There is nothing at this address')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('guide entries on template, network, capability and provider screens have distinct names and destinations in both languages', async ({ page }) => {
  await signIn(page);
  const paths = [
    ...['overview', 'spec', 'access'].map((tab) => `/resources/templates/${RUNS_ON.defaultTemplate}?tab=${tab}`),
    ...['overview', 'ports', 'settings'].map((tab) => `/resources/environments/${RUNS_ON.sandbox}?tab=${tab}`),
    '/admin/capabilities', '/approvals',
    ...['overview', 'models', 'settings'].map((tab) => `/resources/providers/${RUNS_ON.openAiKey}?tab=${tab}`),
  ];
  for (const locale of ['en', 'zh-CN'] as const) {
    if (locale === 'zh-CN') await inChinese(page);
    for (const path of paths) {
      await page.goto(path);
      await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
      await expectUniqueGuideEntries(page);
      await expect(page.locator('[data-help-article]').filter({ hasText: /^(How this works|这是如何工作的)$/ })).toHaveCount(0);
    }
  }
});
