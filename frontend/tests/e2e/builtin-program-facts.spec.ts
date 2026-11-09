/** An empty real built-in thread explains its declared execution needs before a normal fork. */
import { expect, test } from '@playwright/test';
import type { BuiltinThreadProgram } from '../../src/generated/api-types';
import { language } from './ask-support';
import { ACME, DOUBLE, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`built-in thread facts stay distinct from an unbound native thread (${locale})`, async ({ page, request }) => {
  // EN: open the empty built-in thread > Read-only questions > No environment
  // needed > Fork > review its Private copy > Cancel > the source stays open.
  // 中文：打开空的内置线程 > 只读问答 > 无需环境 > 分叉 > 查看私有副本 > 取消，来源保持打开。
  await scenario();
  const program: BuiltinThreadProgram = { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false };
  expect((await request.post(`${DOUBLE}/__double/thread/emit`, { data: { thread: ACME.mainThread, rows: {
    thread: { program, binding: null, environment: null, environment_permissions: [], current_segment_id: null },
  } } })).ok()).toBe(true);
  const t = await language(page, request, locale, 'light');
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-composer-control="program"]')).toHaveText(t('ask.threadProgram'));
  await expect(page.getByText(t('ask.noEnvironment'), { exact: true })).toBeVisible();
  await page.getByRole('button', { name: t('duplicate.now'), exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText(t('ask.privateThread'), { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: t('common.cancel'), exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}`));
  await expect(page.getByText(t('ask.threadProgramDescription'), { exact: true })).toBeVisible();
});
