/** Provider interface pages ship in Help with real schema assets and worked code, without changing a provider. */
import { expect, test } from '@playwright/test';

import { scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

test('Build your own opens the provider contract and its facts/model/reset/presentation pages', async ({ page }) => {
  await signIn(page);
  const changes: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== 'GET' && /\/providers(?:\/|$)/.test(path)) changes.push(`${request.method()} ${path}`);
  });
  await page.goto('/help');
  await page.getByRole('tab', { name: 'Build your own', exact: true }).click();
  await page.getByRole('button', { name: /^Build your own provider source/ }).click();
  await expect(page.getByRole('heading', { name: 'Build your own provider source', level: 1 })).toBeVisible();
  await expect(page.getByRole('link', { name: 'facts', exact: true })).toBeVisible();
  await page.getByRole('link', { name: 'facts', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Implement a provider facts source', level: 1 })).toBeVisible();
  await expect(page.locator('pre').filter({ hasText: 'class Acme:' }).first()).toBeVisible();
  const schemaLink = page.getByRole('link', { name: 'SourceAsk', exact: true });
  const href = await schemaLink.getAttribute('href');
  expect(href).toContain('source-ask');
  expect(href).not.toContain('/help/');
  if (href === null) throw new Error('SourceAsk has no shipped schema link');
  const response = await page.request.get(href);
  expect(response.ok()).toBe(true);
  const schema: unknown = await response.json();
  expect(schema).toMatchObject({ properties: { provider: expect.anything(), config: expect.anything(), secrets: expect.anything() } });

  for (const [id, title] of [
    ['provider-models', 'Implement a provider models source'],
    ['provider-resets', 'Implement provider reset consumption'],
    ['provider-credential', 'Implement key presentation'],
  ] as const) {
    await page.goto(`/help/${id}`);
    await expect(page.getByRole('heading', { name: title, level: 1 })).toBeVisible();
  }
  await page.goto('/help');
  await page.getByRole('searchbox', { name: 'Search the help' }).fill('FactsReport');
  await expect(page.getByRole('button', { name: /^Implement a provider facts source/ })).toBeVisible();
  expect(changes).toEqual([]);
});
