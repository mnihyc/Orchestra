import { afterEach, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { bindingReason, runsLine } from '@/features/start/words';
import { i18n, setLocale } from '@/i18n';
import { options } from './start-fixtures';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`names the selected model and localized effort without routing or metadata separators (${locale})`, async () => {
  await setLocale(locale);
  const answer = options();
  const resolved = resolve(answer, NO_PICKS, { suggest: true });
  const words = runsLine(resolved, i18n.t);
  expect(words).toContain('M1');
  expect(words).toContain(i18n.t('choiceCatalog.effort.low.name'));
  expect(words).toContain('Env e1');
  expect(words).not.toContain('·');
  expect(words).not.toContain('m1');
  expect(resolved.model?.model_name).toBe('m1');
});

for (const locale of ['en', 'zh-CN'] as const) it(`names a refused effort as the effort, not as a refused model (${locale})`, async () => {
  await setLocale(locale);
  const refusal = (reason: string) => new ApiError({ type: 'orch:error:model.not_allowed', code: 'model.not_allowed', title: 'Not allowed', status: 409, detail: 'not allowed', request_id: 'r1', reason }, null);
  expect(bindingReason(refusal('effort'), i18n.t)).toBe(i18n.t('start.refusedEffort'));
  // A refused model keeps the sentence of its code.
  expect(bindingReason(refusal('model'), i18n.t)).toBeNull();
});
