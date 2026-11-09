/** The real native-change entrance exposes destination choices for a no-environment source. */
import { expect, test } from '@playwright/test';
import type { BuiltinThreadProgram } from '../../src/generated/api-types';
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
