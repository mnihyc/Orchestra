/** The actual profile flow keeps explicit choices when the machine's report changes. */
import { expect, type Request, type Route, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function report(harnesses: { kind: string; path: string; version: string; daemon: boolean }[]) {
  const response = await fetch(`${DOUBLE}/__double/resources/machine-programs`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ machine_id: RUNS_ON.laptop, harnesses, discovered_harnesses: harnesses, searched_harness_kinds: ['codex', 'claude-code'] }) });
  if (!response.ok) throw new Error(`the machine report fixture refused: ${response.status}`);
}
const codex = { kind: 'codex', path: '/usr/local/bin/codex', version: '0.162.1', daemon: false };
const claude = { kind: 'claude-code', path: '/usr/local/bin/claude', version: '2.1.296', daemon: false };
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`profile detection ${language}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await report([codex, claude]); await page.setViewportSize({ width, height: 1000 }); await page.goto('/resources/agents');
      await page.getByRole('button', { name: words.agents.list.add, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: words.agents.add.title, exact: true });
      await dialog.getByRole('combobox', { name: words.agentCreation.mode, exact: true }).click();
      await page.getByRole('option', { name: words.agentCreation.onMachine, exact: true }).click();
      await dialog.getByRole('combobox', { name: words.agentCreation.machine, exact: true }).click();
      await page.getByRole('option', { name: 'ada-laptop', exact: true }).click();
      const program = dialog.getByRole('combobox', { name: words.agents.add.harness, exact: true });
      await expect(program).toHaveValue('Codex');
      const facts = dialog.getByLabel(words.agentDetection.title, { exact: true });
      await expect(facts).toContainText('0.162.1'); await expect(facts).toContainText(words.agentDetection.automatic);
      await program.click(); await page.getByRole('option', { name: 'Claude Code', exact: true }).click();
      const name = `Reviewed profile ${language} ${theme} ${width}`;
      await dialog.getByRole('textbox', { name: words.agents.add.name, exact: true }).fill(name);
      const writes: string[] = [];
      const watch = (request: Request) => {
        if (request.method() === 'POST' && new URL(request.url()).pathname === `/api/v1/machines/${RUNS_ON.laptop}/agents`) writes.push(request.url());
      };
      page.on('request', watch);
      await report([codex]);
      await expect(program).toHaveValue('Claude Code');
      await expect(dialog.getByText(words.agentDetection.unavailable, { exact: true })).toBeVisible();
      const blocked = dialog.getByRole('button', { name: words.agents.add.submit, exact: true });
      await expect(blocked).toBeDisabled(); await blocked.focus(); await blocked.press('Enter'); expect(writes).toEqual([]);
      await report([{ ...claude, version: '2.1.297' }, codex]);
      await expect(dialog.getByText(words.agentDetection.unavailable, { exact: true })).toHaveCount(0);
      await expect(program).toHaveValue('Claude Code'); await expect(dialog.getByLabel(words.agentDetection.title, { exact: true })).toContainText('2.1.297');
      await expect(dialog.getByRole('textbox', { name: words.agents.add.name, exact: true })).toHaveValue(name);
      await expect(dialog.getByText(words.agentDetection.yourChoice, { exact: true })).toBeVisible();
      const bounds = await dialog.boundingBox(); if (bounds === null) throw new Error('the real profile dialog is not visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`profile-detection-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      // The client sends its body as a stream, which only an intercepted request shows.
      const bodies: unknown[] = [];
      const created = `/api/v1/machines/${RUNS_ON.laptop}/agents`;
      const capture = async (route: Route) => { if (route.request().method() === 'POST') bodies.push(route.request().postDataJSON()); await route.continue(); };
      const matches = (url: URL) => url.pathname === created;
      await page.route(matches, capture);
      const ack = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === created);
      await dialog.getByRole('button', { name: words.agents.add.submit, exact: true }).click();
      expect((await ack).status()).toBe(200); expect(bodies).toHaveLength(1); expect(bodies[0]).toMatchObject({ name, harness_kind: 'claude-code' });
      await page.unroute(matches, capture);
      await expect(dialog).toHaveCount(0); await expect(page.getByRole('heading', { level: 1, name, exact: true })).toBeVisible();
      expect(writes).toHaveLength(1); page.off('request', watch);
    }
  });
}
