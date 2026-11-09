/** Inline and dialog creation admit their reader choice with the first input, without a later sharing act. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { requestsOf, setDefaults } from './start-support.ts';
import { ACME, scenario, threadHeld } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`admits the environment reader choice from both start entries ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await setDefaults(ACME.fixLogin, {
      'agent.default_provider_id': RUNS_ON.openAiKey,
      'agent.default_agent_id': RUNS_ON.codex,
      'agent.model': 'gpt-5.4',
      'agent.effort': 'high',
      'env.thread_default': 'shared',
      'threads.let_viewers_in': true,
    });
    await signInScreenRoute(page, { width, theme, locale });
    const permissionReads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && /\/(threads|environments)\/[^/]+\/permissions$/u.test(path)) permissionReads.push(path);
    });
    const words = (key: string) => caption(`@${key}`, locale);
    for (const entry of ['quick', 'dialog'] as const) {
      await page.goto(`/topic/${ACME.fixLogin}`);
      const starter = page.locator('[data-quick-start]');
      const message = locale === 'en' ? `Start from ${entry}` : `从${entry === 'quick' ? '快捷输入' : '对话框'}开始`;
      const viewers = entry === 'dialog';
      if (entry === 'quick') {
        const checkbox = starter.getByRole('checkbox', { name: words('access.general.letIn'), exact: true });
        await expect(checkbox).toBeChecked();
        await checkbox.uncheck();
        await expect(checkbox).not.toBeChecked();
        const box = starter.getByRole('textbox', { name: words('start.quick.label.topic'), exact: true });
        await box.fill(message);
        await expect(starter.getByRole('button', { name: words('start.quick.send'), exact: true })).toBeEnabled();
        await page.screenshot({ path: info.outputPath('inline-reader-choice.png') });
        await box.press('Enter');
      } else {
        await starter.getByRole('button', { name: words('start.quick.change'), exact: true }).click();
        const form = page.getByRole('dialog', { name: words('start.title').replace('{{parent}}', 'Fix login'), exact: true });
        await expect(form.getByRole('checkbox', { name: words('access.general.letIn'), exact: true })).toBeChecked();
        await form.getByRole('button', { name: words('start.options.title'), exact: true }).click();
        await form.getByRole('textbox', { name: words('start.message.title'), exact: true }).fill(message);
        const create = form.getByRole('button', { name: words('start.create'), exact: true });
        await expect(create).toBeEnabled();
        await page.screenshot({ path: info.outputPath('dialog-reader-choice.png') });
        await create.click();
      }
      await expect(page).toHaveURL(/\/thread\/[^/?]+/u);
      await expect(page.getByText(message, { exact: true })).toBeVisible();
      const id = /\/thread\/([^/?]+)/u.exec(page.url())?.[1];
      if (id === undefined) throw new Error('The accepted thread must open');
      const held = await threadHeld(id);
      expect(held.inputs.map((input) => input.content)).toEqual([[{ type: 'text', text: message }]]);
      const starts = await requestsOf('POST', new RegExp(`/topics/${ACME.fixLogin}/threads$`, 'u'));
      expect(starts).toHaveLength(entry === 'quick' ? 1 : 2);
      expect(starts.at(-1)?.body).toMatchObject({
        let_into_environment: viewers,
        environment: { use: RUNS_ON.workspace },
        first_input: { content: [{ type: 'text', text: message }], client_id: expect.any(String) },
      });
      expect(permissionReads).toEqual([]);
      expect(await requestsOf('PATCH', /\/permissions$/u)).toHaveLength(0);
      expect(await requestsOf('POST', /\/(environments|access-requests)$/u)).toHaveLength(0);
    }
  });
}
