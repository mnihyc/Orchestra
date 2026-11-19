/** P1 recovery on the real screen against the fake world; native file-job diagnosis belongs to its engine proof. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { SourceInfo, ThreadInput } from '../../src/generated/api-types';
import { threadInput, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/chat-rows';
import { RUNS_ON } from '../doubles/fixtures';
import { change } from './change-support';
import { bodySent, ACME, emit, scenario, signIn, threadHeld } from './support';

const TEXT = 'hello from scenario 39';
const INPUT = '00000000-0000-4000-8088-000000000001';
const source: SourceInfo = { kind: 'directory', directory: '', repository_url: null, branch: null, head: null, state: 'pending', job: null, reason: null };
async function failed(): Promise<ThreadInput> {
  await scenario(); await change.mode('manual');
  await change.rebind(ACME.mainThread, RUNS_ON.sandbox);
  await change.environment({ id: RUNS_ON.sandbox, status: 'failed' });
  const input: ThreadInput = { ...threadInput(INPUT, TEXT, { author: ADA, position: '1' }), thread_id: ACME.mainThread };
  await emit(ACME.mainThread, { thread: { state: 'blocked', lock: null, current_turn_id: null, starting_step: 'environment', source,
    blocked: { reason: 'environment_unavailable', detail: "the environment failed: files: the file job's environment changed", sandbox: null } }, inputs: [input] });
  return input;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) for (const way of ['retry', 'elsewhere'] as const) {
  test(`keeps the queued first input when recovering ${way} (${locale}/${theme}/${width})`, async ({ page }, info) => {
    const input = await failed();
    await page.emulateMedia({ colorScheme: theme }); await page.setViewportSize({ width, height: 900 }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    const words = locale === 'en' ? english : chinese;
    const inputWrites: string[] = [];
    page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.startsWith(`/api/v1/threads/${ACME.mainThread}/inputs`)) inputWrites.push(request.url()); });
    await page.goto(`/thread/${ACME.mainThread}`);
    const recovery = page.locator('[data-environment-start-recovery]');
    await expect(recovery).toBeVisible(); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(recovery).toContainText(words.startRecovery.filesFailed.replace('{{environment}}', 'Sandbox'));
    await expect(recovery).toContainText(words.startRecovery.held);
    await expect(page.getByText(`${words.startRecovery.filesFailed.replace('{{environment}}', 'Sandbox')} ${words.startRecovery.held}`, { exact: true })).toHaveCount(1);
    await expect(page.getByText(TEXT, { exact: true })).toBeVisible();
    await expect(page.getByText(words.source.prepare.pending, { exact: true })).toHaveCount(0);
    await expect(page.getByText(/file job's environment changed/)).toHaveCount(0);
    await info.attach(`failed-setup-${way}-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    if (way === 'retry') {
      const retried = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/environments/${RUNS_ON.sandbox}/start`);
      await recovery.getByRole('button', { name: words.startRecovery.retry, exact: true }).click();
      expect((await retried).ok()).toBe(true);
    } else {
      await recovery.getByRole('button', { name: words.startRecovery.elsewhere, exact: true }).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog).toBeVisible();
      await dialog.getByRole('combobox', { name: words.change.where.label, exact: true }).click();
      await page.getByRole('option', { name: words.changeCompact.destination.use, exact: true }).click();
      await dialog.getByRole('combobox', { name: words.change.where.another, exact: true }).click();
      await page.getByRole('option', { name: /Acme workspace/ }).click();
      // Explicitly choose not to copy from the failed setup in the reviewed plan.
      await dialog.getByRole('radio', { name: words.changeCompact.files.none, exact: true }).click();
      const confirm = dialog.getByRole('button', { name: words.change.confirm.switch, exact: true });
      await expect(confirm).not.toHaveAttribute('aria-disabled', 'true');
      await info.attach(`choose-environment-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      const switched = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/threads/${ACME.mainThread}/forks`);
      await confirm.click();
      const accepted = await switched;
      expect(accepted.ok()).toBe(true);
      expect((await bodySent(accepted.request()))).toMatchObject({ result: 'replace', environment: { use: RUNS_ON.workspace, files: { mode: 'none' } } });
      await change.finish();
      await expect(dialog.locator('[data-job-result="succeeded"]')).toBeVisible();
      await dialog.getByRole('button', { name: words.change.result.doneButton, exact: true }).click();
    }
    const retained = (await threadHeld(ACME.mainThread)).inputs.filter((row) => row.id === INPUT);
    expect(retained).toHaveLength(1); expect(retained[0]).toMatchObject({ state: 'pending', content: input.content });
    expect(inputWrites).toEqual([]);
    // Simulate the native completion frame, preserving the existing input identity. This is not a file-job success proof.
    await emit(ACME.mainThread, { thread: { state: 'idle', starting_step: null, blocked: null, source: { ...source, state: 'ready' } },
      inputs: [{ ...input, state: 'delivered', delivered_at: new Date().toISOString() }],
      items: [userMessage('00000000-0000-4000-8088-000000000002', TEXT, { author: ADA, inputId: INPUT })] });
    await expect(recovery).toHaveCount(0);
    await expect(page.getByText(TEXT, { exact: true })).toHaveCount(1);
    const delivered = (await threadHeld(ACME.mainThread)).inputs.filter((row) => row.id === INPUT);
    expect(delivered).toHaveLength(1); expect(delivered[0]).toMatchObject({ state: 'delivered', content: input.content });
    expect(inputWrites).toEqual([]);
    await info.attach(`recovered-${way}-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });
}
