/** What the server says of an entry that allows more than its giver's own entries do is said in each language. */
import { afterAll, beforeAll, expect, it } from 'vitest';

import { i18n, setLocale, startI18n } from '@/i18n';
import { fieldErrorSentence } from '@/i18n/errors';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

const SENTENCES = {
  purposes: 'allows more uses than any one entry of yours does',
  models: 'allows more models than any one entry of yours does',
  services: 'allows more services than any one entry of yours does',
  efforts: 'allows more efforts than any one entry of yours does',
  arrangements: 'allows more arrangement types than any one entry of yours does',
  where: 'allows more places than any one entry of yours does',
} as const;

beforeAll(async () => { await startI18n('en'); });
afterAll(async () => { await setLocale('en'); });

for (const [locale, catalogue] of [['en', english], ['zh-CN', chinese]] as const) {
  it(`names the member that goes beyond the giver's entries in ${locale}`, async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    for (const [member, sentence] of Object.entries(SENTENCES)) {
      const words = (catalogue.access.options.beyond as Record<string, string>)[member];
      expect(words).toBeTruthy();
      expect(fieldErrorSentence(sentence, t)).toBe(words);
    }
    // Any other sentence of the server is still shown as it is.
    expect(fieldErrorSentence('is a place you cannot see', t)).toBe('is a place you cannot see');
  });
}
