/** A separately reported long-lived write quote stays a source observation, never a silent edit. */
import { expect, it } from 'vitest';
import { modelObservations } from '@/features/resources/providers/modelOrigins';

it('keeps an actual zero twelve-hour quote distinct from an unreported or future-source price', () => {
  const observed = { model_name: 'cache-fixture', display_name: 'Cache fixture', listed: true,
    parameters: { rate: { input_per_mtok: '1', cached_input_per_mtok: '0.1', output_per_mtok: '2', cache_write_12h_per_mtok: '0' } },
    origins: { '/parameters/rate/cache_write_12h_per_mtok': 'vendor' } };
  const original = JSON.stringify(observed);
  expect(modelObservations(observed)).toEqual([{
    pointer: '/parameters/rate/cache_write_12h_per_mtok', field: 'twelveHourCachePrice',
    value: '0', origin: 'vendor', differs: false,
  }]);
  expect(modelObservations({ ...observed, origins: {} })).toEqual([]);
  expect(modelObservations({ ...observed, origins: { '/parameters/rate/cache_write_12h_per_mtok': 'future-source' } })).toEqual([]);
  expect(JSON.stringify(observed)).toBe(original);
});
