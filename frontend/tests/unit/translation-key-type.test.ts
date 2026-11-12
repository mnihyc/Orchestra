/** Metadata keys retain compile-time rejection independently of i18next's per-call resource typing. */
import { expect, it } from 'vitest';
import type { TranslationKey } from '@/i18n';

const known: TranslationKey = 'common.next';
// @ts-expect-error An authored key outside the catalogue must still fail the ordinary compiler.
const unknown: TranslationKey = 'missing.authored.catalogue.key';
it('retains the finite metadata vocabulary while the syntax gate checks translator calls', () => {
  expect(known).toBe('common.next');
  expect(unknown).toBe('missing.authored.catalogue.key');
});
