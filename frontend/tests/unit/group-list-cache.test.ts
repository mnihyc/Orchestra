/** A native group belongs in its org's lists, including the list with departments, and no other org's picker. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { Group } from '@/generated/api-types';

const made: Group = { id: 'made', org_id: 'owner', name: 'Release managers', description: '', member_count: 0 };
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
