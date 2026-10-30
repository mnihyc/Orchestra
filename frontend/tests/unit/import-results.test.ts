/** Import summaries distinguish an unknown outcome from measured zero actions. */
import { expect, it } from 'vitest';
import { importActionSummary } from '@/features/people/importResults';
import { i18n } from '@/i18n';

it('omits zero actions and does not call an unknown outcome zero', () => {
  expect(importActionSummary(null, i18n.t, 'en')).toBe('–');
  expect(importActionSummary({ create: 0, replace: 0, keep_both: 0, skip: 0 }, i18n.t, 'en')).toBe('No changes');
  expect(importActionSummary({ create: 1234, replace: 0, keep_both: 2, skip: 0 }, i18n.t, 'en')).toBe('1,234 created · 2 kept both');
});
