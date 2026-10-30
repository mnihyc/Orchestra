/** Reader warnings use authored sentences and parameters, with the server sentence for an unfamiliar code. */
import { afterEach, expect, it } from 'vitest';

import { readerWarningText } from '@/features/documents/readerWarnings';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });

it.each([
  ['en', 'Picture not found: Maps/site & plan.png'],
  ['zh-CN', '未找到图片：Maps/site & plan.png'],
] as const)('renders the known reader warning with its path in %s', async (locale, sentence) => {
  await setLocale(locale);
  expect(readerWarningText({ code: 'reader.image_missing', params: { path: 'Maps/site & plan.png' }, message: 'Server fallback' }, i18n.t, locale)).toBe(sentence);
});

it.each(['reader.future_warning', 'reader'])('keeps the server sentence for a code without a leaf sentence: %s', async (code) => {
  await setLocale('zh-CN');
  expect(readerWarningText({ code, params: {}, message: 'The source could not be read.' }, i18n.t, 'zh-CN')).toBe('The source could not be read.');
});


it.each([
  ['en', 'There are more of these than are listed'],
  ['zh-CN', '这类问题还有更多，未全部列出'],
] as const)('explains a bounded warning list in %s', async (locale, sentence) => {
  await setLocale(locale);
  expect(readerWarningText({ code: 'reader.more', params: {}, message: 'Server fallback' }, i18n.t, locale)).toBe(sentence);
});
