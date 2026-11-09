/** Separate image allowance buckets and reset windows remain separate timeline notices. */
import { expect, it } from 'vitest';
import { failureKey, sameFailure } from '@/features/timeline/failures';

it('groups only the same image allowance and reset', () => {
  const first = { code: 'image_generation_limit', message: 'Limit reached', window: 'image_gen', resets_at: '2026-10-12T12:00:00Z' };
  const premium = { ...first, window: 'imagegen_premium' };
  const later = { ...first, resets_at: '2026-10-13T12:00:00Z' };
  expect(sameFailure(first, { ...first })).toBe(true);
  expect(sameFailure(first, premium)).toBe(false);
  expect(sameFailure(first, later)).toBe(false);
  expect(failureKey(first)).not.toBe(failureKey(premium));
  expect(failureKey(first)).not.toBe(failureKey(later));
});
