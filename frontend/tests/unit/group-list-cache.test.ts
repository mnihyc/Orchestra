/** A native group belongs in its org's lists, including the list with departments, and no other org's picker. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { Group } from '@/generated/api-types';

const made: Group = { revision: 1, id: 'made', org_id: 'owner', name: 'Release managers', description: '', member_count: 0 };

for (const method of ['POST', 'PATCH', 'DELETE'] as const) it(`repairs only the owner's searched group pages after ${method}`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const here = ['groups', 'owner', 'pages', false, 'Release'], elsewhere = ['groups', 'another-org', 'pages', false, 'Release'];
  const initial = { pages: [{ items: method === 'POST' ? [] : [made], next_cursor: null }], pageParams: [null] };
  const empty = { pages: [{ items: [], next_cursor: null }], pageParams: [null] };
  const read = vi.fn(() => Promise.resolve(method === 'DELETE' ? empty : { pages: [{ items: [made], next_cursor: null }], pageParams: [null] }));
  const otherRead = vi.fn(() => Promise.resolve(empty));
  cache.setQueryData(here, initial); cache.setQueryData(elsewhere, empty);
  const stops = [new QueryObserver(cache, { queryKey: here, queryFn: read }).subscribe(() => undefined), new QueryObserver(cache, { queryKey: elsewhere, queryFn: otherRead }).subscribe(() => undefined)];
  try {
    await applyChanges(findAct(method, method === 'POST' ? '/orgs/owner/groups' : '/groups/made', { answer: method === 'DELETE' ? undefined : made })!.changes, cache);
    expect(read).toHaveBeenCalledOnce(); expect(otherRead).not.toHaveBeenCalled();
    expect(cache.getQueryData(elsewhere)).toEqual(empty);
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it('finds the owning service page through its nested account before deleting that row', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const key = ['service-accounts', 'owner', 'pages', ''];
  const empty = { pages: [{ items: [], next_cursor: null }], pageParams: [null] };
  cache.setQueryData(key, { pages: [{ items: [{ user: { id: 'service' } }], next_cursor: null }], pageParams: [null] });
  const read = vi.fn(() => Promise.resolve(empty));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('DELETE', '/service-accounts/service')!.changes, cache);
    expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(key)).toEqual(empty);
  } finally { stop(); cache.clear(); }
});
for (const under of [false, true]) it(`inserts the accepted group in its owner with departments ${under ? 'included' : 'excluded'} without a read`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const key = ['groups', 'owner', under];
  cache.setQueryData(key, []);
  const read = vi.fn(() => Promise.resolve([]));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('POST', '/orgs/owner/groups', { answer: made })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([made]);
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});

it('does not insert a new group in another org picker', async () => {
  const cache = new QueryClient();
  const key = ['org-groups', 'other-org'];
  cache.setQueryData(key, []);
  try {
    await applyChanges(findAct('POST', '/orgs/owner/groups', { answer: made })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([]);
  } finally { cache.clear(); }
});
