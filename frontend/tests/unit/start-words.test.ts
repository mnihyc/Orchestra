/** A reader of Chinese waits for the Chinese catalogue alone; English, the fallback, comes when the browser is idle. */
import { afterEach, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { bindingReason, runsLine } from '@/features/start/words';
import { options } from './start-fixtures';

import { i18n, loadSearchCatalogues, setLocale, startI18n } from '@/i18n';

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
  expect(bindingReason(refusal('model'), i18n.t)).toBeNull();
});

it('starts with the reader\'s first words and leaves full catalogues to an explicit first use', { timeout: 60_000 }, async () => {
  try {
    // Starting again begins from no words at all, as a new page does.
    await startI18n('zh-CN');
    expect(i18n.hasResourceBundle('zh-CN', 'translation')).toBe(true);
    expect(i18n.hasResourceBundle('en', 'translation')).toBe(false);
    expect(i18n.exists('settings.interface.reading.title')).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(i18n.hasResourceBundle('en', 'translation')).toBe(false);
    await loadSearchCatalogues();
    expect(i18n.hasResourceBundle('en', 'translation')).toBe(true);
    expect(i18n.exists('settings.interface.reading.title')).toBe(true);
  } finally { await setLocale('en'); }
});
