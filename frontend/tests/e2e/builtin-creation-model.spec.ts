/** Real native creation controls and permanent program choices use the generated contracts. */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import type { AskMessageBody, BuiltinThreadProgram, CreateThread, Thread } from '../../src/generated/api-types';
import { askModels } from '../unit/ask-fixtures';
import { id } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { language } from './ask-support';
import { ACME, DOUBLE, scenario } from './support';

const program: BuiltinThreadProgram = { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false };
async function models(page: Page) {
  await page.route('**/api/v1/ask/models*', (route) => route.fulfill({ json: askModels(ACME.org) }));
}
async function publish(request: APIRequestContext, fields: Partial<Thread>) {
  expect((await request.post(`${DOUBLE}/__double/thread/emit`, { data: { thread: ACME.mainThread, rows: { thread: {
    program, binding: null, environment: null, environment_permissions: [], current_segment_id: null, current_turn_id: null,
    state: 'idle', blocked: null, lock: null, ...fields,
  } } } })).ok()).toBe(true);
}
function created(body: CreateThread): Thread {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A genuine native thread fixture is required');
  return { ...world.threads.thread(person, row), program, binding: null, environment: null, environment_permissions: [],
    program_model: body.program?.model ?? null, state: 'idle', current_turn_id: null, current_segment_id: null, lock: null };
}

for (const locale of ['en', 'zh-CN'] as const) test(`native New thread starts a built-in question without workspace choices (${locale})`, async ({ page, request }) => {
  // EN: topic > type first question > Change > Start read-only questions >
  // retain the resolved default and Private > Create > lost reply > retained question > Create again > thread.
  // 中文：话题 > 输入首个问题 > 更改 > 开始只读问答 > 保留已解析的默认模型及私有 > 创建 >
  // 回答丢失 > 问题仍保留 > 再次创建 > 会话。真实原生 API 用例验证创建及第一轮的事务行为。
  await scenario(); await models(page);
  const t = await language(page, request, locale, 'light');
  const births: { body: CreateThread; key: string | undefined }[] = [];
  await page.route(`**/api/v1/topics/${ACME.fixLogin}/threads`, async (route) => {
    const body = route.request().postDataJSON() as CreateThread;
    births.push({ body, key: route.request().headers()['idempotency-key'] });
    if (births.length === 1) { await route.abort('connectionreset'); return; }
    await publish(request, { program_model: body.program?.model ?? null });
    await route.fulfill({ status: 201, json: created(body) });
  });
  await page.goto(`/topic/${ACME.fixLogin}`);
  const quick = page.locator('[data-quick-start]');
  await quick.getByRole('textbox').fill('Explain the current topic');
  await quick.getByRole('button', { name: t('start.quick.change'), exact: true }).click();
  const native = page.getByRole('dialog');
  await native.getByRole('button', { name: t('ask.startProgram'), exact: true }).click();
  const form = page.locator('[data-program-thread]');
  await expect(form.getByText(t('ask.noEnvironment'), { exact: true })).toBeVisible();
  await expect(form.getByRole('textbox', { name: t('start.message.title') })).toHaveValue('Explain the current topic');
  await expect(form.getByRole('combobox', { name: t('start.where.title') })).toHaveCount(0);
  await form.getByRole('button', { name: t('ask.model'), exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Readable API model', exact: true }).click();
  await page.keyboard.press('Escape');
  await form.getByRole('combobox', { name: t('ask.programAudience'), exact: true }).click();
  await page.getByRole('option', { name: t('ask.programPrivate'), exact: true }).click();
  const create = page.getByRole('dialog').getByRole('button', { name: t('start.create'), exact: true });
  await create.click(); await expect.poll(() => births.length).toBe(1);
  await expect(form.getByRole('textbox', { name: t('start.message.title') })).toHaveValue('Explain the current topic');
  await expect(create).toBeEnabled(); await create.click();
  await expect.poll(() => births.length).toBe(2);
  expect(births[0]).toEqual(births[1]); expect(births[0]?.key).toBeTruthy();
  expect(births[0]?.body).toMatchObject({ program: { id: 'orch' }, access: 'private', first_input: { content: [{ type: 'text', text: 'Explain the current topic' }] } });
  expect(births[0]?.body).not.toHaveProperty('program.model');
  for (const field of ['binding', 'environment', 'agent_access', 'directory', 'model', 'effort']) expect(births[0]?.body).not.toHaveProperty(field);
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-program-composer]')).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) test(`permanent thread keeps the selected model and explicitly resets to default (${locale})`, async ({ page, request }) => {
  // EN: permanent built-in thread > API model > select allowed model > Send >
  // current default > Send > reload. The real thread retains a null preference.
  // Default names its resolved model before the picker is opened, including after reload.
  // 中文：永久内置会话 > API 模型 > 选择可用模型 > 发送 > 当前默认模型 > 发送 > 刷新；
  // 真实会话保留空偏好，后续使用组织的当前默认模型。
  // 打开选择器之前就显示默认模型的名称，刷新后也如此。
  await scenario(); await models(page); await publish(request, { program_model: null });
  const t = await language(page, request, locale, 'light');
  const sends: AskMessageBody[] = [];
  await page.route(`**/api/v1/ask/conversations/${ACME.mainThread}/messages`, async (route) => {
    const body = route.request().postDataJSON() as AskMessageBody;
    sends.push(body);
    await publish(request, { program_model: body.model ?? null });
    await route.fulfill({ status: 202, json: { conversation_id: ACME.mainThread, turn_id: id(0x76, sends.length), seq: 100 + sends.length } });
  });
  await page.goto(`/thread/${ACME.mainThread}`);
  const box = page.locator('[data-program-composer]').getByRole('textbox', { name: t('ask.composer') });
  await expect(page.getByRole('button', { name: t('ask.model'), exact: true })).toContainText(`${t('ask.defaultModel')} · Readable API model`);
  await page.getByRole('button', { name: t('ask.model'), exact: true }).click();
  await expect(page.getByRole('menuitemradio', { name: 'Excluded API model', exact: true })).toHaveCount(0);
  await page.getByRole('menuitemradio', { name: 'Readable API model', exact: true }).click();
  await page.keyboard.press('Escape');
  await box.fill('Use the explicit model'); await box.press('Enter');
  await expect.poll(() => sends.length).toBe(1); await expect(box).toHaveValue('');
  expect(sends[0]?.model?.model).toBe('ask-readable');
  await page.getByRole('button', { name: t('ask.model'), exact: true }).click();
  await page.getByRole('menuitemradio', { name: t('ask.defaultModel'), exact: true }).click();
  await page.keyboard.press('Escape');
  await box.fill('Use the current default'); await box.press('Enter');
  await expect.poll(() => sends.length).toBe(2); await expect(box).toHaveValue('');
  expect(sends[1]).toHaveProperty('model', null);
  await page.reload();
  await expect(page.getByRole('button', { name: t('ask.model'), exact: true })).toContainText(`${t('ask.defaultModel')} · Readable API model`);
});
