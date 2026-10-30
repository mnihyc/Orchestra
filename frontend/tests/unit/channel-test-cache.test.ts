/** A notifier's receipt does not contain the channel's persisted delivery state. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { systemChannel } from '../component/admin-system-fixtures';

for (const key of [['channels', 'system'], ['notification-channels']]) {
  for (const shown of [true, false]) {
    it(`retakes the ${shown ? 'shown' : 'hidden'} ${key[0]} delivery state without reading another owner`, async () => {
      const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
      const before = systemChannel();
      const accepted = { ...before, last_error: 'The notifier refused this test' };
      const read = vi.fn(() => Promise.resolve([accepted]));
      const otherKey = ['channels', 'another-org'];
      const other = { ...before, id: 'unrelated-channel' };
      const readOther = vi.fn(() => Promise.resolve([other]));
      cache.setQueryData(key, [before]);
      cache.setQueryData(otherKey, [other]);
      const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
      const stopOther = new QueryObserver(cache, { queryKey: otherKey, queryFn: readOther }).subscribe(() => undefined);
      try {
        await applyChanges(findAct('POST', `/notification-channels/${before.id}/test`, { answer: { ok: false, error: accepted.last_error } })!.changes, cache);
        expect(cache.getQueryData(key)).toEqual(shown ? [accepted] : undefined);
        expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
        expect(cache.getQueryData(otherKey)).toEqual([other]);
        expect(readOther).not.toHaveBeenCalled();
      } finally { stop(); stopOther(); cache.clear(); }
    });
  }
}
