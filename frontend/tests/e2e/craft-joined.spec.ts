/** Later screens keep linked hierarchy, chosen disclosures and explicit canonical port decisions. */
import { expect, test, type Page } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function ports(path: string, body: unknown = {}) {
  const reply = await fetch(`${DOUBLE}/__double/ports${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!reply.ok) throw new Error(`ports fixture refused ${path}: ${reply.status}`);
}
test.beforeEach(async () => { await scenario(); });

test('a thread trail keeps its thread ancestor and its own linked leaf in the same title bar at both widths', async ({ page }) => {
  await signIn(page);
  await page.goto(`/trace/${ACME.mainThread}`);
  const path = page.locator('[data-screen-path]');
  await expect(path).toHaveCount(1);
  await expect(path.getByRole('link', { name: 'Main thread', exact: true })).toHaveAttribute('href', `/thread/${ACME.mainThread}`);
  await expect(path.locator('a[aria-current="page"]')).toHaveAttribute('href', `/trace/${ACME.mainThread}`);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(path).toHaveCount(1);
  await expect(path.locator('a[aria-current="page"]')).toBeVisible();
  await expect(path.locator('a[aria-current="page"]')).toHaveAttribute('href', `/trace/${ACME.mainThread}`);
});

test('a trace grouping and closed filters survive another place and a reload without changing the choice', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme/settings/audit');
  await page.getByRole('combobox', { name: 'Group by', exact: true }).click();
  await page.getByRole('option', { name: 'Kind of act', exact: true }).click();
  const filters = page.locator('details').filter({ has: page.locator('summary').filter({ hasText: /^Filters/ }) }).first();
  if (await filters.getAttribute('open') !== null) await filters.locator(':scope > summary').click();
  await expect(filters).not.toHaveAttribute('open');
  await page.goto(`/project/${ACME.web}/settings?section=audit`);
  await expect(page.getByRole('combobox', { name: 'Group by', exact: true })).toContainText('Kind of act');
  await expect(filters).not.toHaveAttribute('open');
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Group by', exact: true })).toContainText('Kind of act');
  await expect(filters).not.toHaveAttribute('open');
});

test('port disclosures resume after return and reload and each request decision names the port with its canonical sign', async ({ page }) => {
  await ports('/set', { scene: 'full' });
  await ports('/request');
  await signIn(page);
  const route = `/resources/environments/${RUNS_ON.sandbox}?tab=ports`;
  await page.goto(route);
  const policy = page.locator('details').filter({ has: page.getByText('Policies and limits', { exact: true }) });
  const listeners = page.getByTestId('port-3000').locator('details').filter({ has: page.getByText('Listeners', { exact: true }) }).first();
  await policy.locator('summary').click();
  await listeners.locator('summary').click();
  await expect(policy).toHaveAttribute('open', '');
  await expect(listeners).toHaveAttribute('open', '');
  await page.goto(`/project/${ACME.web}`);
  await page.goto(route);
  await expect(policy).toHaveAttribute('open', '');
  await expect(listeners).toHaveAttribute('open', '');
  await page.reload();
  await expect(policy).toHaveAttribute('open', '');
  await expect(listeners).toHaveAttribute('open', '');
  const request = page.getByTestId('port-request');
  await expect(request.getByRole('button', { name: 'Allow port 3000 once', exact: true }).locator('[data-action-icon="allow"]')).toBeVisible();
  await expect(request.getByRole('button', { name: 'Deny port 3000', exact: true }).locator('[data-action-icon="deny"]')).toBeVisible();
});

test('a deliberately folded tree ancestor stays folded across another screen, reload and a second window', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const web = (inPage: Page) => inPage.getByRole('treeitem', { name: /^Web project(?: |$)/ });
  await expect(web(page)).toHaveAttribute('aria-expanded', 'true');
  await web(page).focus();
  await page.keyboard.press('ArrowLeft');
  await expect(web(page)).toHaveAttribute('aria-expanded', 'false');
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(web(page)).toHaveAttribute('aria-expanded', 'false');
  await page.reload();
  await expect(web(page)).toHaveAttribute('aria-expanded', 'false');
  const second = await page.context().newPage();
  try {
    await second.goto(`/thread/${ACME.mainThread}`);
    await expect(web(second)).toHaveAttribute('aria-expanded', 'false');
  } finally { await second.close(); }
  await page.getByRole('button', { name: 'View options', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Open to where I am', exact: true }).click();
  await expect(web(page)).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator(`[role="treeitem"][data-tree-row="${ACME.mainThread}"]`)).toHaveAttribute('aria-current', 'page');
});
