/** Personal List/Board parameters, explicit addresses, readable folder scope and saved stars work without moving another open screen. */
import { expect, test, type Page } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const shipped = { view: 'list', status: 'open', tag_ids: [], attention: null, sort: 'activity', q: '', show_archived: false, with_folders: false };
async function view(project: string, value: Record<string, unknown>) {
  const response = await fetch(`${DOUBLE}/__double/day100/view`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ project, view: { ...shipped, ...value } }) });
  expect(response.ok).toBe(true);
}

/** Show archived and With folders are entries of the row's Filters menu; the menu closes when one is chosen. A menu opened
 * while the page is still drawing its row can be closed by that drawing, so it is opened until it stays open. */
async function filter(page: Page, name: 'With folders' | 'Show archived') {
  const menu = page.getByRole('menu', { name: 'Filters' });
  await expect(async () => {
    if (!(await menu.isVisible())) await page.getByTestId('main').getByRole('button', { name: 'Filters', exact: true }).click();
    await expect(menu).toBeVisible({ timeout: 2_000 });
  }).toPass();
  return menu.getByRole('menuitemcheckbox', { name, exact: true });
}
async function expectFilter(page: Page, name: 'With folders' | 'Show archived', on: boolean) {
  await expect(await filter(page, name)).toHaveAttribute('aria-checked', String(on));
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu', { name: 'Filters' })).toHaveCount(0);
}

test.beforeEach(async () => { await scenario(); });

test('the last personal view returns, an explicit List starts clean, and Clear filters saves every shipped parameter', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('radio', { name: 'Board', exact: true }).click();
  await (await filter(page, 'With folders')).click();
  await expect(page.getByText('Saving your view…')).toHaveCount(0);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('radio', { name: 'Board', exact: true })).toBeChecked();
  await expectFilter(page, 'With folders', true);
  await page.goto(`/project/${ACME.web}?view=list`);
  await expect(page.getByRole('radio', { name: 'List', exact: true })).toBeChecked();
  await expectFilter(page, 'With folders', false);
  await page.goto(`/project/${ACME.web}?view=board&sort=name&with_folders=true&archived=true`);
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await expect(page.getByRole('radio', { name: 'List', exact: true })).toBeChecked();
  await expectFilter(page, 'With folders', false);
  await expectFilter(page, 'Show archived', false);
  await expect(page.getByRole('button', { name: 'Sort by: Activity', exact: true })).toBeVisible();
  await expect(page.getByText('Saving your view…')).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('radio', { name: 'List', exact: true })).toBeChecked();
});

test('each device keeps its own previous project view and projects do not share filters or search words', async ({ page }) => {
  await view(ACME.web, { view: 'board', with_folders: true });
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('radio', { name: 'List', exact: true })).toBeChecked();
  const portable:string[]=[];page.on('request',(request)=>{if(request.url().includes('/me/state/project_views/'))portable.push(request.url());});
  await page.getByRole('radio',{name:'Board',exact:true}).click();
  await (await filter(page,'With folders')).click();
  await view(ACME.web, { view: 'list' });
  await view(ACME.shop, { view: 'board', q: 'separate project' });
  await expect(page.getByRole('radio', { name: 'Board', exact: true })).toBeChecked();
  await page.reload();
  await expect(page.getByRole('radio', { name: 'Board', exact: true })).toBeChecked();
  await expectFilter(page,'With folders',true);
  await page.goto(`/project/${ACME.shop}`);
  await expect(page.getByRole('radio',{name:'List',exact:true})).toBeChecked();
  await expectFilter(page,'With folders',false);
  await expect(page.getByTestId('main').getByRole('searchbox')).toHaveValue('');
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('radio',{name:'Board',exact:true})).toBeChecked();
  await expectFilter(page,'With folders',true);
  expect(portable).toEqual([]);
});

test('a star retains the explicit view and Star the place clears only that favorite’s saved view', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=board&with_folders=true`);
  await page.getByRole('button', { name: /^Star / }).click();
  await page.getByRole('menuitem', { name: 'Star this view', exact: true }).click();
  await expect(page.getByRole('link', { name: 'Open starred view' })).toHaveAttribute('href', /view=board.*with_folders=true/);
  await page.getByRole('button', { name: /^Star / }).click();
  await page.getByRole('menuitem', { name: 'Star the place', exact: true }).click();
  await expect(page.getByRole('link', { name: 'Open starred view' })).toHaveCount(0);
});

test('With folders includes actual descendant paths on the list and board without including sibling work', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.shop}?view=list`);
  await expect(page.getByRole('link', { name: 'Cart rewrite', exact: true })).toHaveCount(0);
  await (await filter(page, 'With folders')).click();
  const table = page.getByRole('table', { name: 'Topics' });
  await expect(table.getByRole('link', { name: 'Cart rewrite', exact: true })).toBeVisible();
  await expect(table).toContainText('Checkout');
  await expect(table).toContainText('Payments');
  await expect(table.getByRole('link', { name: 'Acme', exact: true }).first()).toHaveAttribute('href', '/org/acme');
  await expect(table.getByRole('link', { name: 'Fix login', exact: true })).toHaveCount(0);
  await page.getByRole('radio', { name: 'Board', exact: true }).click();
  await expect(page.getByTestId('main').getByRole('link', { name: 'Cart rewrite', exact: true })).toBeVisible();
  await expectFilter(page, 'With folders', true);
  await expect(page.getByTestId('main').getByRole('link', { name: 'Card fees', exact: true })).toBeVisible();
});
