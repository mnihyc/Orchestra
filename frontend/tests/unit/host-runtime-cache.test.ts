/** Runtime acts update the owning host's shown table from the accepted projection. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';

for (const shown of [true, false]) it(`repairs a ${shown ? 'shown' : 'hidden'} runtime list after deletion without reading another host`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['host-runtimes', 'this-host'];
  const other = ['host-runtimes', 'other-host'];
  const row = { id: 'runtime', unit_name: 'unit', state: 'stopped' };
  // The resource detail is mounted before its table; it cannot stand in for that table's rows.
  cache.setQueryData(['host', 'this-host'], { id: 'this-host', name: 'This host' });
  const readHost = vi.fn(() => Promise.resolve({ id: 'this-host', name: 'This host' }));
  const stopHost = new QueryObserver(cache, { queryKey: ['host', 'this-host'], queryFn: readHost }).subscribe(() => undefined);
  cache.setQueryData(key, [row]);
  cache.setQueryData(other, [{ ...row, id: 'unrelated' }]);
  const read = vi.fn(() => Promise.resolve([]));
  const readOther = vi.fn(() => Promise.resolve([]));
  const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  const stopOther = new QueryObserver(cache, { queryKey: other, queryFn: readOther }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('DELETE', '/hosts/this-host/runtimes/runtime')!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(shown ? [] : undefined);
    expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(cache.getQueryData(other)).toEqual([{ ...row, id: 'unrelated' }]);
    expect(readOther).not.toHaveBeenCalled();
    expect(readHost).not.toHaveBeenCalled();
  } finally { stop(); stopOther(); stopHost(); cache.clear(); }
});

it('applies the returned stopped runtime without another list read', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['host-runtimes', 'this-host'];
  const before = { id: 'runtime', unit_name: 'unit', state: 'running' };
  const accepted = { ...before, state: 'stopping' };
  cache.setQueryData(key, [before]);
  const read = vi.fn(() => Promise.resolve([]));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('POST', '/hosts/this-host/runtimes/runtime/stop', { answer: accepted })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([accepted]);
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});
