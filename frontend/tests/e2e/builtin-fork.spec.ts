/** Both product fork entrances use a genuine built-in thread and its Private creation. */
import { expect, test } from '@playwright/test';
import type { AskKeepBody, BuiltinThreadProgram } from '../../src/generated/api-types';
import { language } from './ask-support';
import { ACME, DOUBLE, playScene, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const selected of [false, true]) {
  test(`built-in ${selected ? 'Fork from here' : 'Fork'} keeps one Private creation (${locale})`, async ({ page, request }) => {
    // EN: open a kept built-in thread > Fork, or first answer > Fork from here…
    // > review Private, destination and history > Fork > retry a lost answer >
    // open the new thread. No agent/environment plan or job is requested.
    // 中文：打开保留的内置线程 > 分叉，或首轮回答 > 从这里分叉… > 查看私有、位置及历史 >
    // 分叉 > 回答丢失后重试 > 打开新线程，不请求智能体、环境方案或执行任务。
    await scenario(); await playScene(ACME.mainThread, 'history');
    const program: BuiltinThreadProgram = { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false };
    expect((await request.post(`${DOUBLE}/__double/thread/emit`, { data: { thread: ACME.mainThread, rows: {
      thread: { program, binding: null, environment: null, environment_permissions: [] },
    } } })).ok()).toBe(true);
    const t = await language(page, request, locale, 'light');
    const writes: { body: AskKeepBody; key: string | undefined }[] = [], native: string[] = [];
    page.on('request', (outgoing) => {
      if (new URL(outgoing.url()).pathname.startsWith(`/api/v1/threads/${ACME.mainThread}/forks`)) native.push(outgoing.url());
    });
    await page.route(`**/api/v1/ask/conversations/${ACME.mainThread}/keep`, async (route) => {
      const outgoing = route.request();
      writes.push({ body: outgoing.postDataJSON() as AskKeepBody, key: outgoing.headers()['idempotency-key'] });
      if (writes.length === 1) { await route.fulfill({ status: 503, json: { status: 503, code: 'internal', message: 'The response was lost', request_id: 'fork-response-lost' } }); return; }
      await route.fulfill({ status: 201, json: { conversation_id: ACME.mainThread, thread_id: ACME.repro, topic_id: ACME.fixLogin } });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    if (selected) {
      await page.getByText('The handler reads').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: t('timeline.selection.forkTo'), exact: true }).click();
    } else await page.getByRole('button', { name: t('duplicate.now'), exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText(t('ask.privateThread'), { exact: true })).toBeVisible();
    await expect(dialog.getByRole('combobox', { name: t('ask.destination'), exact: true })).toBeVisible();
    if (selected) await expect(dialog.getByText(t('agentAccess.forkThrough', { turn: 1 }), { exact: true })).toBeVisible();
    const confirm = dialog.getByRole('button', { name: t('change.menu.forkNow'), exact: true });
    await confirm.click(); await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(confirm).toBeEnabled(); await confirm.click();
    await expect(page).toHaveURL(new RegExp(`/thread/${ACME.repro}`));
    expect(writes).toHaveLength(2); expect(writes[0]?.key).toBeTruthy();
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]?.body).toEqual({ topic_id: null, project_id: null, title: null, ...(selected ? { to_turn: 1 } : {}) });
    expect(native).toEqual([]);
  });
}
