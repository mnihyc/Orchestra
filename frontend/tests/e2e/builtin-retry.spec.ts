/** The ordinary stopped-turn Retry control keeps a genuine built-in thread on its native API entrance. */
import { expect, test } from '@playwright/test';
import type { BuiltinThreadProgram, ThreadTurnRetryBody } from '../../src/generated/api-types';
import { id } from '../doubles/fixtures';
import { ADA } from '../doubles/thread-scenes';
import { threadInput } from '../doubles/thread-rows';
import { language } from './ask-support';
import { ACME, DOUBLE, playScene, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`ordinary Retry is available to the built-in program (${locale})`, async ({ page, request }) => {
  // EN: kept built-in thread > stopped answer > Retry. The existing retry
  // operation sends one client identity, without opening native setup or a job.
  // 中文：保留的内置线程 > 已停止的回答 > 重试；现有重试操作发送一个客户端标识，
  // 不打开原生配置或创建执行任务。
  await scenario(); await playScene(ACME.mainThread, 'failures');
  const program: BuiltinThreadProgram = { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false };
  expect((await request.post(`${DOUBLE}/__double/thread/emit`, { data: { thread: ACME.mainThread, rows: {
    thread: { program, binding: null, environment: null, environment_permissions: [], state: 'idle', blocked: null, current_turn_id: null },
  } } })).ok()).toBe(true);
  const t = await language(page, request, locale, 'light');
  const retries: ThreadTurnRetryBody[] = [], paths: string[] = [];
  await page.route('**/api/v1/threads/*/turns/*/retry', async (route) => {
    const outgoing = route.request(), body = outgoing.postDataJSON() as ThreadTurnRetryBody;
    retries.push(body); paths.push(new URL(outgoing.url()).pathname);
    await route.fulfill({ status: 201, json: { input: {
      ...threadInput(id(0x75, 1), 'Try it with the other config.', { author: ADA, position: '5', state: 'delivered', clientId: body.client_id }),
      thread_id: ACME.mainThread, turn_id: id(0x75, 2),
    }, seq: 1000 } });
  });
  await page.goto(`/thread/${ACME.mainThread}`);
  const retry = page.getByRole('button', { name: t('timeline.footer.retry'), exact: true }).last();
  await expect(retry).toBeEnabled(); await retry.focus(); await retry.click();
  await expect.poll(() => retries.length).toBe(1);
  expect(retries[0]?.client_id).toBeTruthy();
  expect(paths[0]).toMatch(new RegExp(`/threads/${ACME.mainThread}/turns/[^/]+/retry$`));
  await expect(page.getByRole('dialog')).toHaveCount(0);
});
