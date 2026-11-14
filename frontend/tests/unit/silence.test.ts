/** How long nothing may come from a running agent before the status line says so is the thread's own setting, two minutes where nothing is set. */
import { expect, it } from 'vitest';
import { DEFAULT_SILENT_AFTER_MS, silentAfterMs } from '@/features/timeline/silence';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import type { ResolvedSetting } from '@/generated/api-types';

const setting = (value: unknown): ResolvedSetting => ({ key: 'thread.silent_after_secs', value }) as unknown as ResolvedSetting;

it('is two minutes, as the registry ships it, until the thread’s settings are read', () => {
  expect(DEFAULT_SILENT_AFTER_MS).toBe(120_000);
  expect(SETTINGS_REGISTRY.find((def) => def.key === 'thread.silent_after_secs')).toMatchObject({ default: 120, schema: { type: 'integer', minimum: 1 } });
  expect(silentAfterMs(undefined)).toBe(DEFAULT_SILENT_AFTER_MS);
  expect(silentAfterMs([])).toBe(DEFAULT_SILENT_AFTER_MS);
});

it('is the number of seconds the thread’s setting names, however small or large', () => {
  expect(silentAfterMs([setting(30)])).toBe(30_000);
  expect(silentAfterMs([setting(1)])).toBe(1_000);
  expect(silentAfterMs([setting(3_600)])).toBe(3_600_000);
});

it('keeps the default for a value no setting could hold', () => {
  for (const value of [0, -5, Number.NaN, '90', null]) expect(silentAfterMs([setting(value)])).toBe(DEFAULT_SILENT_AFTER_MS);
});
