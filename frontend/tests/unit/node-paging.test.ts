/** A full tree and named shares follow every continuation, including empty visible pages. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { settingsKey } from '@/api/settings';
import { fetchTree } from '@/api/nodes';
import { fetchShared } from '@/api/shared';
import { acmeTree, me, org, userRef } from '../doubles/fixtures';
import type { Node, OrgTreeGetReply, SharedListReply } from '@/generated/api-types';

interface RequestOptions { params: { path?: { id: string }; query: { since?: number; cursor?: string } } }
interface Reply { data: OrgTreeGetReply | SharedListReply; response: Response }
const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options: RequestOptions) => Promise<Reply>>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<object>(), api: { GET: calls.get } }));

const readerBefore = client.session.getState();
beforeEach(() => { vi.clearAllMocks(); queryClient.clear(); client.session.setState({ readerVersion: 7 }); });
afterEach(() => { queryClient.clear(); client.session.setState(readerBefore); });

it('collects every full-tree page and retains the first sequence for live repair', async () => {
  const seed = acmeTree()[0]!;
  const first = { ...seed, id: 'first', name: 'First' };
  const last = { ...seed, id: 'last', name: 'Last' };
  const pages: OrgTreeGetReply[] = [
    { seq: 10, since: null, complete: false, next_cursor: 'one', nodes: [{ ...first, content_hidden: false }] },
    { seq: 11, since: null, complete: false, next_cursor: 'two', nodes: [] },
    { seq: 12, since: null, complete: true, next_cursor: null, nodes: [{ ...last, content_hidden: false }] },
  ];
  calls.get.mockImplementation((_, options) => {
    const index = options.params.query.cursor === 'one' ? 1 : options.params.query.cursor === 'two' ? 2 : 0;
    return Promise.resolve({ data: pages[index]!, response: new Response(null, { status: 200 }) });
  });
  const result = await fetchTree('org', null);
  expect(result.nodes.map((row) => row.name)).toEqual(['First', 'Last']);
  expect(result).toMatchObject({ seq: 10, complete: true, next_cursor: null });
  expect(calls.get.mock.calls.map(([, options]) => options.params.query)).toEqual([{}, { cursor: 'one' }, { cursor: 'two' }]);
});

it('preserves a delta overflow so the live tree can request a full reload', async () => {
  calls.get.mockResolvedValue({ data: { seq: 20, since: 10, complete: false, nodes: [] }, response: new Response(null, { status: 200 }) });
  expect(await fetchTree('org', 10)).toMatchObject({ seq: 20, since: 10, complete: false, nodes: [] });
  expect(calls.get).toHaveBeenCalledTimes(1);
});

it('collects a share beyond an empty visible page rather than reporting an empty success', async () => {
  const shared: Node = org('shared', 'outside', 'Shared place', userRef(me(1, 'ada', 'Ada')));
  calls.get.mockImplementation((_, options) => Promise.resolve({
    data: options.params.query.cursor === 'next' ? { items: [shared], partial: false, next_cursor: null } : { items: [], partial: true, next_cursor: 'next' },
    response: new Response(null, { status: 200 }),
  }));
  expect(await fetchShared()).toEqual([shared]);
  expect(calls.get.mock.calls.map(([, options]) => options.params.query)).toEqual([{}, { cursor: 'next' }]);
});


it('primes native tree settings without replacing a newer setting owner', async () => {
  const page: OrgTreeGetReply = { seq: 1, since: null, complete: true, nodes: [], settings: [] };
  calls.get.mockResolvedValue({ data: page, response: new Response(null, { status: 200 }) });
  await fetchTree('org', null); expect(queryClient.getQueryData(settingsKey('org'))).toEqual([]);
  const current = [{ key: 'navigation.order', value: { by: 'name' } }];
  queryClient.setQueryData(settingsKey('org'), current);
  await fetchTree('org', null); expect(queryClient.getQueryData(settingsKey('org'))).toEqual(current);
});

it.each(['replaced', 'cancelled'])('a %s tree reader cannot seed settings from a late opening', async (kind) => {
  let finish!: (reply: Reply) => void;
  calls.get.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const controller = new AbortController(), opened = fetchTree('org', null, controller.signal);
  if (kind === 'replaced') client.session.setState({ readerVersion: 8 }); else controller.abort();
  finish({ data: { seq: 1, since: null, complete: true, nodes: [], settings: [] }, response: new Response(null, { status: 200 }) });
  await expect(opened).rejects.toMatchObject({ name: 'AbortError' });
  expect(queryClient.getQueryData(settingsKey('org'))).toBeUndefined();
});
