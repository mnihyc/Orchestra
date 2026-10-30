/** Destination receipts update their exact owner; a test/deletion omits the persisted list state. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { Destination } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

function fixture() {
  const world = new World(), person = world.people.get('ada')!;
  const list = () => structuredClone(world.settings.handle(person, 'GET', `/nodes/${ACME.web}/destinations`, new URLSearchParams(), {})!.body as Destination[]);
  const make = () => world.settings.handle(person, 'POST', `/nodes/${ACME.web}/destinations`, new URLSearchParams(), {
    channel: 'webhook', name: 'Deploy room', config: { url: 'https://hooks.example/failing/deploy' }, kinds: [], level: 'important',
  })!.body as Destination;
  return { world, person, list, make };
}

for (const change of ['create', 'update'] as const) it(`applies a native destination ${change} without another read`, async () => {
  const { world, person, list, make } = fixture();
  const original = change === 'update' ? make() : null;
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['destinations', ACME.web], before = list();
  cache.setQueryData(key, before);
  const read = vi.fn(() => Promise.resolve(list()));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    const accepted = original === null ? make() : world.settings.handle(person, 'PATCH', `/destinations/${original.id}`, new URLSearchParams(), { enabled: false })!.body as Destination;
    await applyChanges(findAct(original === null ? 'POST' : 'PATCH', original === null ? `/nodes/${ACME.web}/destinations` : `/destinations/${original.id}`, { answer: accepted })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(list());
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});

for (const shown of [true, false]) it(`repairs the ${shown ? 'shown' : 'hidden'} owner after deleting a destination`, async () => {
  const { world, person, list, make } = fixture(), destination = make();
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['destinations', ACME.web], other = ['destinations', ACME.infra];
  cache.setQueryData(key, list()); cache.setQueryData(other, []);
  const read = vi.fn(() => Promise.resolve(list())), readOther = vi.fn(() => Promise.resolve([]));
  const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  const stopOther = new QueryObserver(cache, { queryKey: other, queryFn: readOther }).subscribe(() => undefined);
  try {
    world.settings.handle(person, 'DELETE', `/destinations/${destination.id}`, new URLSearchParams(), {});
    await applyChanges(findAct('DELETE', `/destinations/${destination.id}`)!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(shown ? list() : undefined);
    expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(readOther).not.toHaveBeenCalled();
    expect(cache.getQueryData(other)).toEqual([]);
  } finally { stop(); stopOther(); cache.clear(); }
});

it('retakes the persisted delivery state after a test receipt without reading another owner', async () => {
  const { world, person, list, make } = fixture(), destination = make();
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['destinations', ACME.web];
  cache.setQueryData(key, list());
  const read = vi.fn(() => Promise.resolve(list()));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    const answer = world.settings.handle(person, 'POST', `/destinations/${destination.id}/test`, new URLSearchParams(), {})!.body;
    await applyChanges(findAct('POST', `/destinations/${destination.id}/test`, { answer })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(list());
    expect(read).toHaveBeenCalledTimes(1);
  } finally { stop(); cache.clear(); }
});
