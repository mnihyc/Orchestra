/** Legacy administered placeholders never resurrect a retired admin step or become readable content. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support';
import { ADA } from '../doubles/thread-scenes';

test.beforeEach(async () => { await scenario(); });

test('a document list does not render the retired administered-reference panel from a stale answer', async ({ page }) => {
  const id = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a68';
  await page.route(`**/api/v1/nodes/${ACME.fixLogin}/documents**`, async (route) => {
    await route.fulfill({ json: { items: [{ id, parent_id: ACME.fixLogin, kind: 'document', name: 'Private handbook', owner: ADA, content_hidden: true, private: true }], next_cursor: null } });
  });
  const reads: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (path === `/api/v1/documents/${id}` || path === `/api/v1/documents/${id}/sync`) reads.push(path);
  });
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('tab', { name: 'Documents', exact: true }).first().click();
  const list = page.getByRole('region', { name: 'Documents', exact: true }).first();
  await expect(list.getByRole('region', { name: 'Private', exact: true })).toHaveCount(0);
  await expect(list.locator('[data-document-row]')).toHaveCount(0);
  await expect(list.getByRole('link', { name: 'Private handbook', exact: true })).toHaveCount(0);
  await expect(list.getByRole('button', { name: 'Private handbook', exact: true })).toHaveCount(0);
  expect(reads).toEqual([]);
});
