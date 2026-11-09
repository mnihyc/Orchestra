/** Native Move entrances review MCP deletion and irreversible group ending in both languages. */
import { expect, test, type Page } from '@playwright/test';
import type { CopyDestinationsRequest } from '../../src/api/copyDestinations';
import type { MoveOutBody, MoveOutCopy } from '../../src/api/moveOut';
import { caption } from '../../e2e/capture/open';
import { resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { me, userRef } from '../doubles/fixtures';
import { chats } from './chat-support';
import { ACME, scenario } from './support';
const NEW_ID = '80000000-0000-4000-8000-000000000088';
async function destinations(page: Page) {
  await page.route(/\/api\/v1\/copy-destinations\/judge$/u, async (route) => {
    const request = route.request().postDataJSON() as CopyDestinationsRequest;
    await route.fulfill({ json: { subject: request.subject, intent: request.intent, source_owned: true, destinations: request.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'needs_choice', choice: 'delete_preview' } })) } });
  });
}
for (const mode of SCREEN_PRESENTATIONS) {
  const words = (key: string, args: Record<string, string> = {}) => caption(`@${key}`, mode.locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => args[name] ?? whole);
  test(`MCP Move follows one native source-delete plan (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: MCP servers > menu > Move > other org > source deletion plan > confirm > actual acknowledged identity
    // 中文：MCP 服务器 > 菜单 > 移动 > 其他组织 > 原生删除计划 > 确认 > 打开回执中的实际对象
    await scenario(); await resourceFixtures(page); await destinations(page); await signInScreenRoute(page, mode);
    const writes: MoveOutBody[] = [], deletes: string[] = [];
    await page.route(/\/api\/v1\/mcp-servers\/([^/]+)\/move-out(?:\/preview)?$/u, async (route) => {
      if (new URL(route.request().url()).pathname.endsWith('/preview')) {
        const copy = route.request().postDataJSON() as MoveOutCopy, id = /\/mcp-servers\/([^/]+)/u.exec(route.request().url())?.[1];
        await route.fulfill({ json: { subject: { kind: 'mcp_server', id }, destination: copy.destination, revision: copy.expected_revision ?? 1, stamp: 'native-mcp-delete-footprint', effects: ['remove_source'] } });
      } else { writes.push(route.request().postDataJSON() as MoveOutBody); await route.fulfill({ json: { projection: 'receipt', kind: 'mcp_server', id: NEW_ID } }); }
    });
    page.on('request', (request) => { if (request.method() === 'DELETE') deletes.push(request.url()); });
    await page.goto('/resources/mcp'); await screenReady(page, mode.locale, mode.theme);
    const row = page.getByRole('table').getByRole('row').filter({ has: page.locator('a[href^="/resources/mcp/"]') }).first();
    const name = await row.locator('a[href^="/resources/mcp/"]').first().innerText();
    await row.getByRole('button', { name: words('resources.act.menuOf', { name }), exact: true }).click(); await page.getByRole('menuitem', { name: words('duplicate.move'), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words('duplicate.moveTitle', { name }), exact: true }); await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
    await page.getByRole('option').filter({ hasText: words('duplicate.choices.delete_preview') }).first().click(); await expect(dialog.getByRole('table', { name: words('duplicate.deletePlan'), exact: true })).toBeVisible();
    await expect(dialog.getByRole('combobox', { name: words('duplicate.machine'), exact: true })).toHaveCount(0);
    expect(writes).toEqual([]); await page.mouse.move(0, 0); await page.screenshot({ path: info.outputPath('mcp-native-move-plan.png'), animations: 'disabled' });
    await dialog.getByRole('button', { name: new RegExp(mode.locale === 'en' ? '^Copy to ' : '^复制到') }).click();
    await expect(page).toHaveURL(new RegExp(`/resources/mcp/${NEW_ID}`)); expect(writes).toHaveLength(1); expect(writes[0]?.delete_plan.stamp).toBe('native-mcp-delete-footprint'); expect(writes[0]?.copy.machine_id).toBeNull(); expect(deletes).toEqual([]);
  });
  test(`group Move confirms history ending and native member notices (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
    // EN: group > menu > Move > other org > native ending plan > actual members > typed name > one confirmed move
    // 中文：群聊 > 菜单 > 移动 > 其他组织 > 原生结束计划 > 实际成员 > 输入群名 > 一次确认移动
    await scenario(); const made = await chats('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Move crew' }); if (typeof made.chat !== 'string') throw new Error('The native group fixture omitted its actual id');
    await destinations(page); await signInScreenRoute(page, mode);
    const writes: MoveOutBody[] = [], deletes: string[] = [];
    await page.route(/\/api\/v1\/chats\/([^/]+)\/move-out(?:\/preview)?$/u, async (route) => {
      if (new URL(route.request().url()).pathname.endsWith('/preview')) {
        const copy = route.request().postDataJSON() as MoveOutCopy;
        await route.fulfill({ json: { delete_plan: { subject: { kind: 'chat', id: made.chat }, destination: copy.destination, revision: 7, stamp: 'native-group-ending', effects: ['end_group_and_history'] }, notification: { people: 2, notify_limit: 200, preview_id: 'native-group-members' } } });
      } else { writes.push(route.request().postDataJSON() as MoveOutBody); await route.fulfill({ json: { projection: 'receipt', kind: 'chat', id: NEW_ID } }); }
    });
    await page.route(/\/api\/v1\/operations\/native-group-members\/recipients(?:\?|$)/u, (route) => route.fulfill({ json: { total: 2, hidden: 1, people: { items: [userRef(me(1, 'ada', 'Visible member'))], next_cursor: null } } }));
    page.on('request', (request) => { if (request.method() === 'DELETE') deletes.push(request.url()); });
    await page.goto(`/org/acme/messages?chat=${made.chat}`); await screenReady(page, mode.locale, mode.theme);
    await page.getByRole('button', { name: words('chat.group.menu'), exact: true }).click(); await page.getByRole('menuitem', { name: words('duplicate.move'), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words('duplicate.moveTitle', { name: 'Move crew' }), exact: true }); await dialog.getByRole('combobox', { name: words('duplicate.destination'), exact: true }).click();
    await page.getByRole('option').filter({ hasText: words('duplicate.choices.delete_preview') }).first().click(); await expect(dialog.getByText(words('duplicate.moveEffects.end_group_and_history'), { exact: true })).toBeVisible();
    const confirm = dialog.getByRole('button', { name: new RegExp(mode.locale === 'en' ? '^Copy to ' : '^复制到') }); await expect(confirm).toBeDisabled();
    await dialog.getByRole('button', { name: words('operation.notify', { number: '2' }), exact: true }).click(); await expect(page.getByText('Visible member', { exact: true })).toBeVisible(); await page.keyboard.press('Escape');
    await dialog.getByRole('checkbox').check(); await dialog.getByRole('textbox', { name: words('duplicate.typeName'), exact: true }).fill('Move crew');
    expect(writes).toEqual([]); expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true); await page.mouse.move(0, 0); await page.screenshot({ path: info.outputPath('group-native-ending-review.png'), animations: 'disabled' });
    await confirm.click(); await expect(page).toHaveURL(new RegExp(`/chat/${NEW_ID}`)); expect(writes).toHaveLength(1); expect(deletes).toEqual([]);
    expect(writes[0]).toMatchObject({ delete_plan: { revision: 7, stamp: 'native-group-ending' }, notify: true, notify_preview_id: 'native-group-members' });
  });
}
