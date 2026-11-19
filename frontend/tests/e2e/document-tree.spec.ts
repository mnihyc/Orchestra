/** Real browser controls on production assets, with explicit synthetic folder projections. */
import AxeBuilder from '@axe-core/playwright';
import { writeFile } from 'node:fs/promises';
import { expect, type Page, type TestInfo, test } from '@playwright/test';
import type { Document } from '../../src/generated/api-types';
import { signInWithProjectionReader } from './projection-support.ts';
import { DOCS } from './documents-support.ts';
import { PEOPLE, scenario, signIn } from './support.ts';
import { documentTreeFixture, TREE } from './document-tree-support.ts';

async function capture(page: Page, info: TestInfo, name: string) {
  await page.mouse.move(0, 0); await page.evaluate(() => document.fonts.ready);
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path, fullPage: true, animations: 'disabled' });
  await info.attach(name, { path, contentType: 'image/png' });
}

test.beforeEach(async () => { await scenario(); });
for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`document folders create and copy directly, preview deletion and restore at ${width}/${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const fixture = await documentTreeFixture(page);
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    const read = await signInWithProjectionReader(page);
    fixture.showDocumentFacts(await read<Document>(`/api/v1/documents/${DOCS.plan}`));
    await page.goto('/shared');
    await page.getByRole('table', { name: 'Shared with me' }).getByRole('link', { name: 'Research', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Experiments', exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Plan.md', exact: true })).toBeVisible();
    await capture(page, info, `folder-${width}-${theme}`);
    await page.getByRole('button', { name: 'New folder', exact: true }).click();
    let dialog = page.getByRole('dialog', { name: 'New folder', exact: true });
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Results');
    await dialog.getByRole('button', { name: 'New folder', exact: true }).click();
    await expect(page.getByRole('link', { name: 'Results', exact: true })).toBeVisible();
    expect(fixture.requests.filter((request) => request.body?.command === 'create_folder')).toHaveLength(1);
    await page.getByRole('link', { name: 'Results', exact: true }).click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
    dialog = page.getByRole('dialog', { name: 'Rename', exact: true });
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Findings');
    await dialog.getByRole('button', { name: 'Rename', exact: true }).click();
    await expect(page.getByRole('link', { name: 'Findings', exact: true })).toBeVisible();
    expect(fixture.requests.find((request) => request.body?.name === 'Findings')?.body).toMatchObject({ command: 'move', to: { kind: 'container', id: TREE.research } });
    await page.getByRole('link', { name: 'Plan.md', exact: true }).click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
    dialog = page.getByRole('dialog', { name: 'Rename', exact: true });
    await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('Plan');
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Reviewed plan');
    await dialog.getByRole('button', { name: 'Rename', exact: true }).click();
    await expect(page.getByRole('link', { name: 'Reviewed plan.md', exact: true })).toBeVisible();
    expect(fixture.requests.find((request) => request.body?.name === 'Reviewed plan')?.body).toMatchObject({ command: 'rename', to: null, items: [{ kind: 'document', id: TREE.plan }] });

    expect(fixture.requests.filter((request) => request.path.endsWith('/preview'))).toHaveLength(0);
    await page.getByRole('link', { name: 'Experiments', exact: true }).click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Copy to…', exact: true }).click();
    dialog = page.getByRole('dialog', { name: 'Copy to…', exact: true });
    await dialog.getByRole('combobox', { name: 'Destination', exact: true }).fill('Archive');
    await page.getByRole('option').filter({ hasText: 'Archive' }).click();
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Experiments copy');
    await capture(page, info, `folder-copy-${width}-${theme}`);
    await dialog.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(fixture.requests.filter((request) => request.body?.command === 'copy')).toHaveLength(1);
    expect(fixture.requests.filter((request) => request.path.endsWith('/preview'))).toHaveLength(0);
    await page.goto(`/document-folder/${TREE.archive}`);
    const copied = page.getByRole('link', { name: 'Experiments copy', exact: true });
    await expect(copied).toBeVisible(); await copied.click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    dialog = page.getByRole('dialog', { name: 'Delete', exact: true });
    await expect(dialog.getByText('Experiments copy/Appendix.md', { exact: true })).toBeVisible();
    await expect(dialog.getByText('1 folder, 1 document, 2 attached files and 0 retained versions.', { exact: true })).toBeVisible();
    await expect(dialog.getByText(/Recoverable until/)).toBeVisible();
    expect(fixture.requests.filter((request) => request.path === '/document-tree/operations' && request.body?.command === 'delete')).toHaveLength(0);
    await capture(page, info, `folder-delete-preview-${width}-${theme}`);
    await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(copied).toHaveCount(0);
    const deletion = fixture.requests.filter((request) => request.path === '/document-tree/operations' && request.body?.command === 'delete');
    expect(deletion).toHaveLength(1); expect(deletion[0]?.body?.deletion_plan?.entries).toHaveLength(2);
    await page.getByRole('radio', { name: 'Deleted', exact: true }).click();
    await expect(page.getByText('Experiments copy', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Restore', exact: true }).click();
    await expect(page.getByText('Experiments copy', { exact: true })).toHaveCount(0);
    await page.getByRole('radio', { name: 'Current', exact: true }).click();
    await expect(copied).toBeVisible(); await copied.click();
    await expect(page.getByRole('link', { name: 'Appendix.md', exact: true })).toBeVisible();
    await capture(page, info, `folder-restored-${width}-${theme}`);
    const accessibility = await new AxeBuilder({ page }).analyze();
    expect(accessibility.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')).toEqual([]);
    expect(errors).toEqual([]);
    await writeFile(info.outputPath('folder-requests.json'), JSON.stringify(fixture.requests, null, 2));
  });
}

test('a folder-only deep link stays quiet, takes live names, and erases revoked content without opening its private node', async ({ page }, info) => {
  const fixture = await documentTreeFixture(page);
  const privateReads: string[] = [];
  page.on('request', (request) => { if (new URL(request.url()).pathname === `/api/v1/nodes/${TREE.node}`) privateReads.push(request.url()); });
  await signIn(page); await page.goto(`/document-folder/${TREE.research}`);
  await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Plan.md', exact: true })).toBeVisible();
  const before = fixture.requests.length;
  const research = fixture.rows.get(TREE.research); if (research === undefined) throw new Error('The fixture folder is required');
  research.entry.title = research.entry.basename = 'Field research'; fixture.broadcast();
  await expect(page.getByRole('heading', { name: 'Field research', exact: true })).toBeVisible();
  expect(fixture.requests).toHaveLength(before);
  // One real idle minute, with no timer mocking or directory refresh.
  await page.waitForTimeout(60_000);
  expect(fixture.requests).toHaveLength(before);
  fixture.revoke();
  await expect(page.getByRole('link', { name: 'Plan.md', exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Field research', exact: true })).toHaveCount(0);
  await capture(page, info, 'folder-access-withdrawn');
  expect(privateReads).toEqual([]);
});


test('retained folder ancestors stay in inspection, including a live parent, without a live 404', async ({ page }, info) => {
  const fixture = await documentTreeFixture(page);
  const research = fixture.rows.get(TREE.research);
  if (research === undefined) throw new Error('The selected folder is required');
  research.entry.parent = { kind: 'container', id: TREE.archive };
  for (const row of fixture.rows.values()) row.standing.manage = true;
  await signIn(page); await page.goto(`/document-folder/${TREE.research}`);
  await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Research', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete', exact: true });
  await expect(dialog.getByText('Research/Experiments/Appendix.md', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/inspect/document_container/${TREE.research}`));
  await page.getByText('Experiments', { exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Look at it', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/inspect/document_container/${TREE.nested}`));
  const before = fixture.requests.length;
  await expect(page.getByRole('link', { name: 'Research', exact: true })).toHaveAttribute('href', `/inspect/document_container/${TREE.research}`);
  await page.getByRole('link', { name: 'Research', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toHaveCount(0);
  await page.getByRole('link', { name: 'Archive', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/inspect/document_container/${TREE.archive}`));
  await expect(page.getByRole('link', { name: 'Open current folder', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder', exact: true })).toHaveCount(0);
  expect(fixture.requests.slice(before).filter((request) => request.path.startsWith('/document-tree/entries/') && !request.query.includes('inspect_deleted=true'))).toEqual([]);
  await capture(page, info, 'retained-parent-inspection');
});


test('selected-folder rename uses its parent and Move keeps its root scope while Copy can choose another org', async ({ page }) => {
  const fixture = await documentTreeFixture(page);
  const research = fixture.rows.get(TREE.research), archive = fixture.rows.get(TREE.archive);
  if (research === undefined || archive === undefined) throw new Error('Folder fixtures are required');
  research.entry.parent = { kind: 'container', id: TREE.archive };
  for (const row of fixture.rows.values()) row.standing.manage = true;
  const outside = structuredClone(archive);
  outside.entry.item = { kind: 'container', id: '00000000-0000-4000-8091-000000000090' };
  outside.entry.root = '00000000-0000-4000-8091-000000000091'; outside.entry.node = '00000000-0000-4000-8091-000000000092';
  outside.entry.parent = { kind: 'node_docs', id: outside.entry.node }; outside.entry.title = outside.entry.basename = 'Other org folder';
  fixture.rows.set(outside.entry.item.id, outside);
  await signIn(page); await page.goto(`/document-folder/${TREE.research}`);
  await page.getByRole('button', { name: 'Actions for Research', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  let dialog = page.getByRole('dialog', { name: 'Rename', exact: true });
  await expect(dialog.getByRole('button', { name: 'Rename', exact: true })).toBeDisabled();
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Field research');
  await dialog.getByRole('button', { name: 'Rename', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Field research', exact: true })).toBeVisible();
  expect(fixture.requests.find((request) => request.body?.name === 'Field research')?.body).toMatchObject({ command: 'move', items: [{ kind: 'container', id: TREE.research }], to: { kind: 'container', id: TREE.archive } });
  await page.getByRole('link', { name: 'Experiments', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Move to…', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Move to…', exact: true });
  await dialog.getByRole('combobox', { name: 'Destination', exact: true }).fill('Other org');
  await expect.poll(() => fixture.requests.filter((request) => request.path === '/search/names').at(-1)?.query.includes(`org_id=${research.entry.root}`)).toBe(true);
  await expect(page.getByRole('option').filter({ hasText: 'Other org folder' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Move', exact: true })).toBeDisabled();
  await dialog.getByRole('combobox', { name: 'Destination', exact: true }).fill('Archive');
  await page.getByRole('option').filter({ hasText: 'Archive' }).click();
  await expect(dialog.getByRole('button', { name: 'Move', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('link', { name: 'Experiments', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy to…', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Copy to…', exact: true });
  await dialog.getByRole('combobox', { name: 'Destination', exact: true }).fill('Other org');
  await page.getByRole('option').filter({ hasText: 'Other org folder' }).click();
  await expect(dialog.getByRole('button', { name: 'Copy', exact: true })).toBeEnabled();
  expect(fixture.requests.filter((request) => request.path.endsWith('/preview'))).toEqual([]);
});


test('an open document follows a move into a folder and renamed breadcrumbs without reloading content or reading the private node', async ({ page }, info) => {
  const fixture = await documentTreeFixture(page);
  const row = fixture.rows.get(TREE.plan);
  if (row === undefined) throw new Error('Document metadata is required');
  row.standing = { read: true, write: false, manage: false };
  row.entry.parent = fixture.root;
  const bodyReads: string[] = [], privateReads: string[] = [], privateTrees: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (path === `/api/v1/documents/${TREE.plan}/sync`) bodyReads.push(path);
    if (path === `/api/v1/nodes/${TREE.node}`) privateReads.push(path);
    if (path === `/api/v1/orgs/${row.entry.root}/tree`) privateTrees.push(path);
  });
  await page.route(`**/api/v1/documents/${TREE.plan}/sync*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as { document: Document };
    body.document.node = { id: TREE.node, kind: 'project', name: null, redacted: true };
    body.document.path = [{ id: row.entry.root, kind: 'org', name: null, redacted: true }, body.document.node];
    body.document.location = { parent: row.entry.parent, basename: row.entry.basename, ancestors: [] };
    await route.fulfill({ response, json: body });
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page, PEOPLE.vera); privateTrees.length = 0; await page.goto(`/doc/${TREE.plan}`);
  await expect(page.getByRole('textbox', { name: 'Document text', exact: true })).toBeVisible();
  await expect.poll(() => fixture.requests.some((request) => request.path === `/document-tree/entries/${TREE.plan}`)).toBe(true);
  await expect(page.locator('article').getByRole('link', { name: 'Research', exact: true })).toHaveCount(0);
  const count = bodyReads.length;
  row.entry.parent = { kind: 'container', id: TREE.research }; fixture.broadcast();
  await expect(page.locator('article').getByRole('link', { name: 'Research', exact: true })).toBeVisible();
  const folder = fixture.rows.get(TREE.research); if (folder === undefined) throw new Error('Folder metadata is required');
  folder.entry.title = folder.entry.basename = 'Field research'; fixture.broadcast();
  const parent = page.locator('article').getByRole('link', { name: 'Field research', exact: true });
  await expect(parent).toBeVisible(); expect(bodyReads).toHaveLength(count); expect(privateReads).toEqual([]); expect(privateTrees).toEqual([]);
  await capture(page, info, 'nested-document-current-location');
  await parent.click();
  await expect(page.getByRole('heading', { name: 'Field research', exact: true })).toBeVisible();
  expect(privateReads).toEqual([]);
});
