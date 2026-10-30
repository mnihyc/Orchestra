/** Rate-setting receipts omit resolved model rows, which only the shown native rate table may supply. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const [setting, value] of [['usage.catalogue', false], ['usage.catalogue_scale', '2']] as const) it(`retakes the shown rate table after ${setting} without reading another scope`, async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const key = ['usage', 'rates', ACME.org], hidden = ['usage', 'rates', ACME.web], other = ['usage', 'rates', 'another-root'];
  const table = () => world.usage.handle(person, 'GET', '/usage/rates', new URLSearchParams({ node_id: ACME.org }), {})!.body;
  cache.setQueryData(other, 'unrelated');
  cache.setQueryData(key, table());
  cache.setQueryData(hidden, table());
  cache.setQueryData(['node', ACME.web], { id: ACME.web, path: [{ id: ACME.org }] });
  const read = vi.fn(() => Promise.resolve(table())), readOther = vi.fn(() => Promise.resolve('unrelated'));
  const stopOther = new QueryObserver(cache, { queryKey: other, queryFn: readOther }).subscribe(() => undefined);
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    const path = `/nodes/${ACME.org}/settings/${setting}`;
    const reply = world.usage.handle(person, 'PUT', path, new URLSearchParams(), { value, expected_revision: 0 })!;
    await applyChanges(findAct('PUT', path, { answer: reply.body })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(table());
    expect(read).toHaveBeenCalledTimes(1);
    expect(readOther).not.toHaveBeenCalled();
    expect(cache.getQueryData(hidden)).toBeUndefined();
  } finally { stop(); stopOther(); cache.clear(); }
});
