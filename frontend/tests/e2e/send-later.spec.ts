/** Own human Send later click paths in idle/running threads, both languages, themes and widths. */
import { expect, test, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures';
import { SCREEN_PRESENTATIONS } from '../../e2e/layout-routes';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { ACME, DOUBLE, playScene, scenario } from './support';
import type { InputScheduleCreateBody, InputScheduleUpdateBody } from '../../src/generated/api-types';
import { inputSchedule } from '../doubles/input-schedules';
import { askModels } from '../unit/ask-fixtures';
import { ADA } from '../doubles/chat-rows';

async function bounded(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  for (const panel of await page.getByRole('dialog').all()) {
    const box = await panel.boundingBox(); expect(box).not.toBeNull();
    if (box !== null) { expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual((page.viewportSize()?.width ?? 0) + 1); }
  }
}
for (const mode of SCREEN_PRESENTATIONS) for (const running of [false, true]) test(`Send later keeps the full draft without sending now (${running ? 'running' : 'idle'}/${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: thread > complete draft > Send menu > Send later > cancel > reopen > explicit Once > failed save > retry > accepted
  // 中文：线程 > 完整草稿 > 发送菜单 > 稍后发送 > 取消 > 重新打开 > 明确选择一次时间 > 保存失败 > 重试 > 接受
  const words = (key: string) => caption(`@${key}`, mode.locale);
  await scenario(); await resourceFixtures(page);
  // Both the initial page and the live thread feed must report the intended state and turn.
  await playScene(ACME.mainThread, running ? 'running' : 'history');
  await signInScreenRoute(page, mode);
  const writes: { body: InputScheduleCreateBody; key: string | undefined }[] = [], immediate: string[] = [];
  await page.route(/\/api\/v1\/topics\/[^/]+\/input-schedules$/u, async (route) => {
    if (route.request().method() !== 'POST') { await route.continue(); return; }
    writes.push({ body: route.request().postDataJSON() as InputScheduleCreateBody, key: route.request().headers()['idempotency-key'] });
    if (writes.length === 1) { await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: '', status: 403, request_id: 'scheduled-refusal' } }); return; }
    await route.fulfill({ status: 201, json: { projection: 'receipt', schedule_id: '01950000-0000-7000-8000-000000000011', revision: 1, topic_seq: 2 } });
  });
  page.on('request', (request) => { if (request.method() === 'POST' && /\/api\/v1\/(?:threads\/[^/]+\/inputs|topics\/[^/]+\/threads|environments)$/u.test(new URL(request.url()).pathname)) immediate.push(request.url()); });
  await page.goto(`/thread/${ACME.mainThread}`); await screenReady(page, mode.locale, mode.theme);
  await expect(page.getByRole('group', { name: words('composer.row.label'), exact: true })
    .getByRole('button', { name: words(running ? 'composer.stop' : 'composer.send'), exact: true })).toBeVisible();
  const composer = page.getByRole('textbox', { name: words('composer.label'), exact: true });
  await composer.fill('Every word of this authored message\nAnd its second line');
  const open = async () => {
    await page.getByRole('button', { name: words('composer.deliveryMenu'), exact: true }).click();
    await page.getByRole('menuitem', { name: words('sendLater.title'), exact: true }).click();
    return page.getByRole('dialog', { name: words('sendLater.title'), exact: true });
  };
  let dialog = await open();
  await expect(dialog.getByRole('textbox', { name: words('sendLater.date'), exact: true })).toHaveValue('');
  await expect(dialog.locator('input[type=time]')).toHaveValue('');
  await bounded(page); await page.screenshot({ path: info.outputPath('send-later-empty-time.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: words('common.cancel'), exact: true }).click(); expect(writes).toEqual([]);
  await expect(composer).toHaveValue('Every word of this authored message\nAnd its second line');
  dialog = await open();
  await dialog.getByRole('textbox', { name: words('sendLater.date'), exact: true }).fill('2030-02-01');
  await dialog.locator('input[type=time]').fill('09:30');
  // Enter in a field/IME never submits the schedule.
  await dialog.locator('input[type=time]').dispatchEvent('keydown', { key: 'Enter', isComposing: true }); expect(writes).toEqual([]);
  await dialog.getByRole('button', { name: words('common.save'), exact: true }).click(); await expect(dialog.getByRole('alert')).toBeVisible();
  // The modal retires background accessibility, but the original composer's complete draft must remain.
  await expect(page.getByRole('textbox', { name: words('composer.label'), exact: true, includeHidden: true })).toHaveValue('Every word of this authored message\nAnd its second line');
  await bounded(page); await page.screenshot({ path: info.outputPath('send-later-refusal-kept.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: words('common.save'), exact: true }).click(); await expect(dialog).toHaveCount(0);
  expect(writes).toHaveLength(2); expect(writes[1]).toEqual(writes[0]); expect(writes[0]?.key).toBeTruthy();
  expect(writes[0]?.body.content).toContainEqual({ type: 'text', text: 'Every word of this authored message\nAnd its second line' });
  expect(writes[0]?.body.target).toEqual({ kind: 'thread', thread_id: ACME.mainThread }); expect(immediate).toEqual([]);
  await expect(composer).toHaveValue('');
});

for (const mode of SCREEN_PRESENTATIONS) test(`human schedule editing and resume use the shown revision (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: thread Schedules > Edit > retained private draft > Save > Resume; no immediate input
  // 中文：线程的定时计划 > 编辑 > 保留的私有草稿 > 保存 > 继续；不立即发送输入
  const words = (key: string) => caption(`@${key}`, mode.locale);
  await scenario(); await resourceFixtures(page);
  let row = inputSchedule({ kind: null, agent: null, author: ADA, state: 'paused', topic: { id: ACME.fixLogin, kind: 'topic', name: 'Login', redacted: false }, destination: { thread: { id: ACME.mainThread, kind: 'thread', name: 'Review', redacted: false }, new_thread: false }, definition: { target: { kind: 'thread', thread_id: ACME.mainThread }, content: [{ type: 'text', text: 'A private scheduled prompt' }], native_action: null } });
  const supply = async () => {
    const response = await fetch(`${DOUBLE}/__double/request-feeds/topic-schedules`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ topic: ACME.fixLogin, rows: [row] }) });
    expect(response.ok).toBe(true);
  };
  await supply(); await signInScreenRoute(page, mode);
  const writes: InputScheduleUpdateBody[] = [];
  await page.route(/\/api\/v1\/input-schedules\/[^/]+$/u, async (route) => {
    if (route.request().method() !== 'PATCH') { await route.fulfill({ status: 200, json: row }); return; }
    const body = route.request().postDataJSON() as InputScheduleUpdateBody; writes.push(body);
    row = { ...row, revision: row.revision + 1, topic_seq: row.topic_seq + 1, name: body.name ?? row.name, state: body.state ?? row.state, definition: row.definition === null ? null : { ...row.definition, content: body.content ?? row.definition.content } };
    await supply();
    await route.fulfill({ status: 200, json: row });
  });
  await page.goto(`/thread/${ACME.mainThread}`); await screenReady(page, mode.locale, mode.theme);
  const { showWorkbenchView } = await import('./workbench-support'); await showWorkbenchView(page, 'Schedules');
  const table = page.getByRole('table', { name: words('agentSchedules.title'), exact: true });
  await table.getByRole('button', { name: words('sendLater.edit'), exact: true }).click();
  const dialog = page.getByRole('dialog', { name: words('sendLater.editTitle'), exact: true });
  await expect(dialog.getByRole('textbox', { name: words('sendLater.message'), exact: true })).toHaveValue('A private scheduled prompt');
  await dialog.getByRole('textbox', { name: words('sendLater.message'), exact: true }).fill('An explicitly changed prompt');
  await dialog.getByRole('textbox', { name: words('sendLater.name') }).fill('Edited schedule');
  await bounded(page); await page.screenshot({ path: info.outputPath('edit-private-schedule.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: words('common.save'), exact: true }).click(); await expect(dialog).toHaveCount(0);
  expect(writes[0]).toMatchObject({ revision: 1, name: 'Edited schedule', content: [{ type: 'text', text: 'An explicitly changed prompt' }], state: null });
  // Resume uses the new native row only after the edited name proves that projection has reached the list.
  await expect(table.getByText('Edited schedule', { exact: true })).toBeVisible();
  await table.getByRole('button', { name: words('sendLater.resume'), exact: true }).click();
  await expect.poll(() => writes.length).toBe(2); expect(writes[1]).toMatchObject({ revision: 2, state: 'active', content: null, target: null, timing: null });
});

for (const mode of SCREEN_PRESENTATIONS) test(`Send later program setup prepares without an immediate birth (${mode.locale}/${mode.theme}/${mode.width})`, async ({ page }, info) => {
  // EN: Send later > New thread > Configure > program > Use these choices > reopen > Cancel; draft intact, no thread created
  // 中文：稍后发送 > 新线程 > 配置 > 内置程序 > 使用这些选择 > 重新打开 > 取消；草稿保留，不立即创建线程
  const words = (key: string) => caption(`@${key}`, mode.locale);
  await scenario(); await resourceFixtures(page); await signInScreenRoute(page, mode);
  await page.route('**/api/v1/ask/models*', (route) => route.fulfill({ json: askModels(ACME.org) }));
  const births: string[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && /\/api\/v1\/(?:topics\/[^/]+\/threads|environments)$/u.test(new URL(request.url()).pathname)) births.push(request.url()); });
  await page.goto(`/thread/${ACME.mainThread}`); await screenReady(page, mode.locale, mode.theme);
  const composer = page.getByRole('textbox', { name: words('composer.label'), exact: true }); await composer.fill('A scheduled program question');
  await page.getByRole('button', { name: words('composer.deliveryMenu'), exact: true }).click();
  await page.getByRole('menuitem', { name: words('sendLater.title'), exact: true }).click();
  const sheet = page.getByRole('dialog', { name: words('sendLater.title'), exact: true });
  await sheet.getByRole('combobox', { name: words('sendLater.destination'), exact: true }).click();
  await page.getByRole('option', { name: words('sendLater.newThread'), exact: true }).click();
  await sheet.getByRole('checkbox', { name: words('sendLater.topicDefaults'), exact: true }).uncheck();
  await sheet.getByRole('button', { name: words('sendLater.configure'), exact: true }).click();
  await page.getByRole('button', { name: words('ask.startProgram'), exact: true }).click();
  let program = page.getByRole('dialog').filter({ has: page.locator('[data-program-thread]') });
  await program.getByRole('textbox', { name: words('start.options.name'), exact: true }).fill('Later program');
  await expect(program.getByRole('textbox', { name: words('start.message.title'), exact: true })).toHaveCount(0);
  expect(births).toEqual([]); await program.getByRole('button', { name: words('sendLater.useSetup'), exact: true }).click();
  await sheet.getByRole('button', { name: words('sendLater.configure'), exact: true }).click();
  program = page.getByRole('dialog').filter({ has: page.locator('[data-program-thread]') });
  await expect(program.getByRole('textbox', { name: words('start.options.name'), exact: true })).toHaveValue('Later program');
  await bounded(page); await page.mouse.move(0, 0); await page.screenshot({ path: info.outputPath('program-schedule-preparation.png'), animations: 'disabled' });
  await program.getByRole('button', { name: words('common.cancel'), exact: true }).click();
  await sheet.getByRole('button', { name: words('common.cancel'), exact: true }).click();
  expect(births).toEqual([]); await expect(composer).toHaveValue('A scheduled program question');
});
