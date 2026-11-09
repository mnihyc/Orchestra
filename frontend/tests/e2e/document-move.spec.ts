/** Each presentation walks the actual document folder menu through one native cross-org operation. */
import { expect, test } from '@playwright/test';
import type { CopyDestinationsRequest } from '../../src/api/copyDestinations';
import type { DocumentMoveBody } from '../../src/api/documentMove';
import type { DocumentTreePage } from '../../src/generated/api-types';
import { WAY_SEPARATOR } from '../../src/features/finder/destinations';
import { caption } from '../../e2e/capture/open';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { id, me, org, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { documentTreeFixture, TREE } from './document-tree-support';
import { scenario } from './support';
const DESTINATION = id(92, 1), NEW_ROOT = id(92, 2), CHILD = id(92, 3);
for (const mode of SCREEN_PRESENTATIONS) test(`document folder native Move out (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Folder > menu > Move > other org > complete native deletion plan > cancel > reopen > one Move-out act
  // 中文：文件夹 > 菜单 > 移动 > 其他组织 > 完整原生删除计划 > 取消 > 重新打开 > 一次移出操作
  const words = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  await scenario(); const fixture = await documentTreeFixture(page); for (const row of fixture.rows.values()) row.standing.manage = true;
  const other = org(DESTINATION, 'other', 'Other org', userRef(me(1, 'ada', 'Ada')));
  await page.route(/\/api\/v1\/orgs$/u, (route) => route.fulfill({ json: [other] }));
  await page.route(new RegExp(`/api/v1/orgs/${DESTINATION}/tree(?:\\?|$)`), (route) => route.fulfill({ json: { seq: 1, since: null, complete: true, hidden: [], nodes: [treeNode({ id: DESTINATION, kind: 'org', parent_id: null, name: 'Other org' })] } }));
  const target: DocumentTreePage = { directory: { at: { kind: 'node_docs', id: DESTINATION }, node: { kind: 'org', id: DESTINATION, name: 'Other org', redacted: false }, path: [{ kind: 'org', id: DESTINATION, name: 'Other org', redacted: false }], folder: null, ancestors: [], can_create: true, inspection: null, membership_seq: 1 }, items: [], ancestors: [], next_cursor: null, feed: 'native-target-docs' };
  await page.route(new RegExp(`/api/v1/nodes/${DESTINATION}/document-tree(?:\\?|$)`), (route) => route.fulfill({ json: target }));
  await page.route(/\/api\/v1\/copy-destinations\/judge$/u, async (route) => {
    const body = route.request().postDataJSON() as CopyDestinationsRequest;
    await route.fulfill({ json: { subject: body.subject, intent: body.intent, source_owned: true, destinations: body.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'needs_choice', choice: 'delete_preview' } })) } });
  });
  const writes: DocumentMoveBody[] = [], keys: (string | undefined)[] = [], deletes: string[] = [];
  await page.route(/\/api\/v1\/document-tree\/operations$/u, async (route) => {
    const body = route.request().postDataJSON() as DocumentMoveBody; writes.push(body); keys.push(route.request().headers()['idempotency-key']);
    await route.fulfill({ json: { projection: 'receipt', operation: 'native-atomic-move', entries: [{ kind: 'container', id: CHILD }, { kind: 'container', id: NEW_ROOT }] } });
  });
  page.on('request', (request) => { if (request.method() === 'DELETE') deletes.push(request.url()); });
  await signInScreenRoute(page, mode); await page.goto(`/document-folder/${TREE.research}`); await screenReady(page, mode.locale, mode.theme);
  const open = async () => {
    await page.getByRole('button', { name: words('files.menu.for', { name: 'Research' }), exact: true }).click();
    await page.getByRole('menuitem', { name: words('documentFolders.actions.move'), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words('documentFolders.actions.move'), exact: true });
    const oldPath = [words('ui.nameUnknown'), words('ui.nameUnknown')].join(WAY_SEPARATOR);
    await dialog.getByRole('button', { name: words('settings.place.clear', { path: oldPath }), exact: true }).click();
    await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
    const option = page.getByRole('option', { name: 'Other org', exact: true }); await expect(option).not.toHaveAttribute('aria-disabled', 'true'); await option.click();
    await expect(dialog.getByRole('table', { name: words('duplicate.deletePlan'), exact: true })).toBeVisible(); return dialog;
  };
  let dialog = await open(); await dialog.getByRole('button', { name: words('common.cancel'), exact: true }).click(); expect(writes).toEqual([]);
  dialog = await open(); await expect(dialog).toContainText('Research/Experiments/Appendix.md');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: info.outputPath('native-document-move-plan.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: words('duplicate.moveAcross', { org: 'Other org' }), exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/documents/${DESTINATION}`)); expect(writes).toHaveLength(1); expect(keys[0]).toBeTruthy(); expect(deletes).toEqual([]);
  expect(writes[0]).toMatchObject({ command: 'move_out', to: { kind: 'node_docs', id: DESTINATION }, items: [{ kind: 'container', id: TREE.research }], deletion_plan: { authority_fingerprint: 'fixture-authority' } });
  expect(fixture.requests.some((request) => request.path.endsWith('/delete/preview') && request.body?.command === 'delete')).toBe(true);
});
