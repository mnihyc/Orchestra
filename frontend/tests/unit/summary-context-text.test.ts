import { afterEach, expect, it } from 'vitest';
import { settingFieldName } from '@/features/settings/settingText';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`names the same input token budget in producer and target settings (${locale})`, async () => {
  await setLocale(locale);
  const expected = locale === 'en' ? 'Input tokens' : '输入词元';
  expect(settingFieldName('context_tokens', 'auto.summarizer', i18n.t)).toBe(expected);
  expect(settingFieldName('context_tokens', 'summary.targets', i18n.t)).toBe(expected);
});
