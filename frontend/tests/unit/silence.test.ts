/** How long nothing may come from a running agent before the status line says so is the thread's own setting, two minutes where nothing is set. */
import { expect, it } from 'vitest';
import { silentAfterMs } from '@/features/timeline/silence';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';

it('uses the received native default and makes no silence claim before it is known', () => {
  expect(SETTINGS_REGISTRY.find((def) => def.key === 'thread.silent_after_secs')).toMatchObject({ default: 120, schema: { type: 'integer', minimum: 1 } });
  expect(silentAfterMs(120)).toBe(120_000);
  expect(silentAfterMs(undefined)).toBe(Number.POSITIVE_INFINITY);
});

it('is the number of seconds the thread’s setting names, however small or large', () => {
  expect(silentAfterMs(30)).toBe(30_000);
  expect(silentAfterMs(1)).toBe(1_000);
  expect(silentAfterMs(3_600)).toBe(3_600_000);
});

it('makes no silence claim for a value no setting could hold', () => {
  for (const value of [0, -5, Number.NaN, '90', null]) expect(silentAfterMs(value)).toBe(Number.POSITIVE_INFINITY);
});
