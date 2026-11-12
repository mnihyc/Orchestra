/** A failed family-model read keeps its entrance and retries only the read. */
import { expect, test } from '@playwright/test';
import type { ThreadOpening, ThreadParameters, Turn } from '../../src/generated/api-types';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { USUAL_MODEL } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { ACME, configureThread, emit, scenario, signIn } from './support';
import { chats } from './chat-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`family model read stays visible ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await chats('/locale', { as: 'ada', locale });
    await signIn(page);
    await configureThread(ACME.mainThread, { reset: true });
    const words = locale === 'en' ? en : zh;
    const failed: Turn = { ...turn('family-read-turn', { ordinal: 1, state: 'failed', inputId: 'family-read-input', model: USUAL_MODEL.name }),
      error: { code: 'turn.limit', message: 'A native family limit', retryable: true, said: null, resets_at: null, window: null, incomplete: false,
        limit: { scope: 'model_family', model_family: 'Native family', retry_after_secs: null, window_minutes: null } } };
    await emit(ACME.mainThread, { thread: { state: 'blocked', current_turn_id: null, lock: null, blocked: { reason: 'provider_exhausted', detail: 'A native family limit', sandbox: null } }, turns: [failed] });
    const gate: { release?: () => void } = {};
    const pending = new Promise<void>((resolve) => { gate.release = resolve; });
    let denied = true;
    let alternative: string | undefined;
    let modelWrites = 0;
    page.on('request', (request) => {
      if (request.method() === 'PUT' && new URL(request.url()).pathname.endsWith('/settings/agent.model')) modelWrites++;
    });
    // Exercise the separate parameter reader when the joined opening cannot supply its projection.
    await page.route(`**/api/v1/threads/${ACME.mainThread}/opening?*`, async (route) => {
      const response = await route.fetch();
      const opening = await response.json() as ThreadOpening;
      opening.parameters = null;
      await route.fulfill({ response, json: opening });
    });
    await page.route(`**/api/v1/threads/${ACME.mainThread}/parameters`, async (route) => {
      await pending;
      if (denied) {
        await route.fulfill({ status: 404, json: { type: 'orch:error:not_found', code: 'not_found', title: '', detail: 'Hidden provider or model', status: 404, request_id: 'private-model-read' } });
      } else {
        const response = await route.fetch();
        const parameters = await response.json() as ThreadParameters;
        alternative = parameters.model.options.find((option) => option.allowed && option.model_name !== USUAL_MODEL.name && option.display_name.trim() !== '')?.display_name;
        await route.fulfill({ response, json: parameters });
      }
    });
    try {
      // EN: Open the family-limited thread → disabled model entrance → Try again → choose another model.
      // 中文：打开受模型族限额限制的线程 → 禁用的模型入口 → 重试 → 选择其他模型。
      await page.goto(`/thread/${ACME.mainThread}`);
      const surface = page.locator('[data-family-limit]');
      const picker = surface.getByRole('combobox', { name: words.thread.blocked.familyChoose, exact: true });
      await expect(picker).toBeDisabled();
      await expect(picker).toContainText(words.thread.blocked.familyChecking);
      await page.screenshot({ path: info.outputPath(`model-read-checking-${locale}-${theme}-${width}.png`), fullPage: true });
      gate.release?.();
      await expect(picker).toContainText(words.thread.blocked.familyReadFailed);
      await expect(surface.getByRole('alert')).toBeVisible();
      await expect(surface).not.toContainText('Hidden provider or model');
      expect(modelWrites).toBe(0);
      denied = false;
      await surface.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      await expect(picker).toBeEnabled();
      expect(modelWrites).toBe(0);
      if (alternative === undefined) throw new Error('The native fixture must offer an actual permitted alternative');
      await picker.click();
      await page.getByRole('option', { name: alternative, exact: true }).click();
      await expect.poll(() => modelWrites).toBe(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`model-read-restored-${locale}-${theme}-${width}.png`), fullPage: true });
    } finally { gate.release?.(); }
  });
}
