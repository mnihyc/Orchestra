/** The actual Parameters entrance and schema controls in both languages, widths and themes. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { ResolvedSetting, Thread } from '../../src/generated/api-types';
import { ACME, bodySent, emit, PEOPLE, scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const MAIN = ACME.mainThread;
const INTERFACE = 'openai-responses';
const KEY = `provider_options:${INTERFACE}`;
const SETTING_PATH = `/api/v1/nodes/${MAIN}/settings/agent.params`;

async function control(path: string, body: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fixture refused ${path}: ${response.status}`);
}

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`thread request options ${language}/${theme}`, async ({ page }, info) => {
    await scenario();
    const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: PEOPLE.ada.login, password: PEOPLE.ada.password }) });
    if (!login.ok) throw new Error(`the fixture could not sign in: ${login.status}`);
    const { access_token } = await login.json() as { access_token: string };
    const headers = { Authorization: `Bearer ${access_token}`, 'Content-Type': 'application/json' };
    const read = await fetch(`${DOUBLE}/api/v1/nodes/${MAIN}`, { headers });
    if (!read.ok) throw new Error(`the fixture could not read its thread: ${read.status}`);
    const original = await read.json() as Thread;
    if (original.binding === null || original.model === null) throw new Error('the fixture needs its native bound thread');
    await control('/model/entry', { provider_id: original.binding.provider.id, model_name: original.model, display_name: 'Thread option model', parameters: {
      request_options: { [INTERFACE]: { type: 'object', additionalProperties: false, properties: {
        mode: { type: 'string', title: 'Mode', enum: ['a', 'b'] }, region: { title: 'Region', const: 'local' },
      } } },
    } });
    await emit(MAIN, { thread: { state: 'idle', binding: { ...original.binding, isolation: { ...original.binding.isolation, credential_path: 'forwarded' } } } });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    const settings = async () => {
      const response = await fetch(`${DOUBLE}/api/v1/nodes/${MAIN}/settings`, { headers });
      if (!response.ok) throw new Error(`the fixture could not read its settings: ${response.status}`);
      const rows = await response.json() as ResolvedSetting[];
      const row = rows.find((row) => row.key === 'agent.params');
      if (row === undefined) throw new Error('the actual agent parameter setting is missing');
      return row;
    };
    for (const width of [1280, 400]) {
      const seen = await settings();
      const reset = await fetch(`${DOUBLE}${SETTING_PATH}`, { method: 'PUT', headers, body: JSON.stringify({
        value: { [KEY]: { mode: 'a' }, native_kept: true, 'provider_options:another-interface': { untouched: true } }, expected_revision: seen.own?.revision ?? 0,
      }) });
      expect(reset.status).toBe(200);
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/thread/${MAIN}`);
      await showWorkbenchView(page, 'Parameters');
      const panel = page.getByTestId('parameters-panel');
      const section = panel.getByRole('region', { name: words.inspector.parameters.group.provider, exact: true });
      const row = section.locator(`[data-provider-interface="${INTERFACE}"]`);
      await expect(row.getByRole('textbox', { name: 'Region', exact: true })).toHaveValue('local');
      await expect(row.getByRole('textbox', { name: 'Region', exact: true })).toBeDisabled();
      await expect(row.getByText(words.providerOptions.fixedByProvider, { exact: true })).toBeVisible();
      const guide = section.getByRole('button', { name: words.inspector.parameters.optionsGuide, exact: true });
      await expect(guide).toHaveAttribute('data-help-heading', 'thread-options');
      const choice = row.getByRole('combobox', { name: 'Mode', exact: true });
      const before = await settings();
      const sent = page.waitForRequest((request) => request.method() === 'PUT' && new URL(request.url()).pathname === SETTING_PATH);
      const acknowledged = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === SETTING_PATH);
      await choice.click();
      await page.getByRole('option', { name: 'b', exact: true }).click();
      expect(await bodySent(await sent)).toMatchObject({ expected_revision: before.own?.revision ?? 0, value: {
        [KEY]: { mode: 'b' }, native_kept: true, 'provider_options:another-interface': { untouched: true },
      } });
      expect((await acknowledged).status()).toBe(200);
      await expect.poll(async () => (await settings()).own?.value).toEqual({
        [KEY]: { mode: 'b' }, native_kept: true, 'provider_options:another-interface': { untouched: true },
      });
      await expect(choice).toHaveValue('b');
      const bounds = await row.boundingBox();
      if (bounds === null) throw new Error('the actual thread option row is not visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`thread-options-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      const cleared = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === SETTING_PATH);
      await row.getByRole('button', { name: words.inspector.parameters.reset.replace('{{name}}', INTERFACE), exact: true }).click();
      expect((await cleared).status()).toBe(200);
      await expect.poll(async () => (await settings()).own?.value).toEqual({ native_kept: true, 'provider_options:another-interface': { untouched: true } });
      await guide.click();
      await expect(page.getByRole('heading', { name: language === 'en' ? 'Thread options' : '线程选项', exact: true })).toBeVisible();
    }
  });
}
