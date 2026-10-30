import { expect, it } from 'vitest';
import { NO_PICKS } from '@/features/start/choices';
import { readLastSetup } from '@/features/start/lastSetup';

it('keeps only typed setup choices, leaving every native authority check to the current options', () => {
  expect(readLastSetup(JSON.stringify({ place: 'topic', picks: { ...NO_PICKS, provider: 'provider', model: 'native-route', effort: 'high', environment: 'environment', arrangement: 'separated' }, password: 'never restored', message: 'not a saved draft' }))).toEqual({ place: 'topic', picks: { ...NO_PICKS, provider: 'provider', model: 'native-route', effort: 'high', environment: 'environment', arrangement: 'separated' } });
});

it.each(['', 'null', '[]', '{', '{"place":12,"picks":{}}', '{"place":"topic","picks":null}', '{"place":"topic","picks":{"arrangement":"unknown"}}'])('ignores corrupt or incompatible saved setup %s', (raw) => {
  expect(readLastSetup(raw)).toBeNull();
});
