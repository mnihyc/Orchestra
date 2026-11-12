/** The real native-change entrance exposes destination choices for a no-environment source. */
import { expect, test } from '@playwright/test';
import type { BuiltinThreadProgram, ForkPlan } from '../../src/generated/api-types';
import { askModels } from '../unit/ask-fixtures';
import { language } from './ask-support';
import { ACME, DOUBLE, playScene, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['switch', 'fork'] as const) {
  test(`built-in native ${kind} requires a destination and offers no source workspace (${locale})`, async ({ page, request }) => {
    // EN: kept Orch thread > Switch provider or agent… / Duplicate to… > inspect the
    // real environment picker > New environment > cancel. No source files or
    // volumes are offered, and no incomplete plan or creation is submitted.
    // 中文：已保留的 Orch 会话 > 切换提供方或智能体… / 复制到… > 查看实际环境选择器 >
    // 新环境 > 取消；不提供不存在的来源文件或卷，不提交未完成的预检或创建。
    await scenario(); await playScene(ACME.mainThread, 'history');
    await page.setViewportSize({width: 1440, height: 900});
    const program: BuiltinThreadProgram = {id: 'orch', root_org_id: ACME.org, read_only: true,
      requires_environment: false, requires_native_harness: false};
    expect((await request.post(`${DOUBLE}/__double/thread/emit`, {data: {thread: ACME.mainThread, rows: {
      thread: {program, binding: null, environment: null, environment_permissions: []},
    }}})).ok()).toBe(true);
    const t = await language(page, request, locale, 'light');
    const submissions: string[] = [];
    page.on('request', (outgoing) => {
      const path = new URL(outgoing.url()).pathname;
      if (outgoing.method() === 'POST' && (path.startsWith(`/api/v1/threads/${ACME.mainThread}/forks`)
        || path === `/api/v1/ask/conversations/${ACME.mainThread}/keep`)) submissions.push(path);
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.getByRole('button', {name: t(kind === 'switch' ? 'change.menu.switch' : 'duplicate.to'), exact: true}).click();
    const dialog = page.getByRole('dialog').filter({has: page.locator('[data-thread-change-pickers]')});
    await expect(dialog.getByRole('combobox', {name: t('change.runs.provider'), exact: true})).toBeVisible();
    const where = dialog.getByRole('combobox', {name: t('change.where.label'), exact: true});
    await expect(where).toBeVisible();
    await expect(dialog.getByRole('button', {name: t(kind === 'switch' ? 'change.confirm.switch' : 'change.confirm.fork'), exact: true}))
      .toHaveAttribute('aria-disabled', 'true');
    await expect(dialog.getByText(t('changeReady.chooseNativeBinding'), {exact: true})).toBeVisible();
    if (await page.getByRole('listbox').isVisible()) await page.keyboard.press('Escape');
    await where.click();
    await expect(page.getByRole('option', {name: t('change.where.sameNone'), exact: true})).toHaveCount(0);
    await page.getByRole('option', {name: t('start.where.createNew'), exact: true}).click();
    await expect(dialog.getByRole('combobox', {name: t('change.where.place'), exact: true})).toBeVisible();
    await expect(dialog.getByRole('radiogroup', {name: t('fork.files.title'), exact: true})).toHaveCount(0);
    await expect(dialog.getByRole('checkbox', {name: t('fork.files.home'), exact: true})).toHaveCount(0);
    await expect(dialog.getByRole('checkbox', {name: t('fork.files.privateViews'), exact: true})).toHaveCount(0);
    expect(submissions).toEqual([]);
    await dialog.getByRole('button', {name: t('common.cancel'), exact: true}).click();
    await expect(dialog).toHaveCount(0);
    expect(submissions).toEqual([]);
  });
}

for (const locale of ['en', 'zh-CN'] as const) test(`reviews an actual program source before a native switch (${locale})`, async ({page, request}) => {
  // EN: kept read-only thread > Switch provider or agent > choose provider,
  // profile, model and an existing environment > review Program and workspace > Cancel.
  // 中文：已保留的只读会话 > 切换提供方或智能体 > 选择提供方、配置、模型及已有环境 >
  // 查看程序和工作区 > 取消。方案来源是内置程序，不得伪造来源配置或工作区。
  await scenario(); await playScene(ACME.mainThread, 'history');
  const model = askModels(ACME.org).default_model;
  if (model === null) throw new Error('The source must name its actual configured model');
  const program: BuiltinThreadProgram = {id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false};
  expect((await request.post(`${DOUBLE}/__double/thread/emit`, {data: {thread: ACME.mainThread, rows: {
    thread: {program, program_model: model, binding: null, environment: null, environment_permissions: []},
  }}})).ok()).toBe(true);
  const t = await language(page, request, locale, 'light');
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', {name: t('change.menu.switch'), exact: true}).click();
  const dialog = page.getByRole('dialog').filter({has: page.locator('[data-thread-change-pickers]')});
  const chooseFirst = async (label: string) => {
    await dialog.getByRole('combobox', {name: label, exact: true}).click();
    await page.getByRole('option', {disabled: false}).first().click();
  };
  await chooseFirst(t('change.runs.provider'));
  await chooseFirst(t('change.runs.agent'));
  await expect(dialog.getByRole('combobox', {name: t('change.runs.model'), exact: true})).toBeVisible();
  await dialog.getByRole('combobox', {name: t('change.where.label'), exact: true}).click();
  await page.getByRole('option', {name: t('start.where.useExisting'), exact: true}).click();
  const preview = page.waitForResponse((response) => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `/api/v1/threads/${ACME.mainThread}/forks/preflight` && response.ok());
  await chooseFirst(t('change.where.another'));
  const plan = await (await preview).json() as ForkPlan;
  expect(plan.from.kind).toBe('program');
  expect(plan.from).not.toHaveProperty('agent'); expect(plan.from).not.toHaveProperty('environment');
  expect(plan.files).toBeNull(); expect(plan.leaves).toBeNull();
  await expect(dialog.locator('[data-plan-list="changes"]')).toContainText(t('ask.threadProgram'));
  await expect(dialog.locator('[data-plan-list="changes"]')).toContainText(plan.to.agent.name);
  await dialog.getByRole('button', {name: t('common.cancel'), exact: true}).click();
  await expect(dialog).toHaveCount(0);
});
