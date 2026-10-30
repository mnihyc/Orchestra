/** A restored root repairs its native page projections; an acknowledged system agent keeps native list membership. */
import { InfiniteQueryObserver, QueryClient, QueryObserver, type QueryKey } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { client, type Act } from '@/api/client';
import type { Org, OrgPage } from '@/generated/api-types';
import { me, org, userRef } from '../doubles/fixtures';
import { detailAgent } from '../component/resource-detail-fixtures';

const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
const reader = client.session.getState(), releases: (() => void)[] = [];
afterEach(() => { for (const release of releases.splice(0)) release(); cache.clear(); client.session.setState(reader); });
const before = org('restored', 'old-labs', 'Old Labs', userRef(me(1, 'keeper', 'Keeper')));
const restored: Org = { ...before, archived: true, archived_at: '2026-10-18T12:00:00Z', revision: before.revision + 1 };
const page = (items: Org[], next_cursor: string | null = null): OrgPage => ({ items, next_cursor });
const key = (deleted: boolean) => ['admin', 'orgs', { q: '', deleted }] as const;
function watch<T>(queryKey: QueryKey, data: T, read: () => Promise<T>) {
  cache.setQueryData(queryKey, data);
  const observer = new QueryObserver(cache, { queryKey, queryFn: read, staleTime: Infinity });
  releases.push(observer.subscribe(() => undefined)); return observer;
}
async function accepted(act: Act) {
  const found = findAct(act.method, act.path, act); if (found === null) throw new Error('The native act is unregistered');
  await applyChanges(found.changes, cache);
}
const restore = () => accepted({ method: 'POST', path: '/nodes/restored/restore', answer: restored });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

it('repairs live/deleted pages once with their original cursor chain while personal and unrelated readers remain quiet', async () => {
  const sibling = { ...before, id: 'sibling', name: 'A sibling' }, nested = { ...restored, id: 'nested', parent_id: restored.id, depth: 1 };
  cache.setQueryData(key(false), { pages: [page([sibling], 'native-cursor'), page([])], pageParams: [null, 'native-cursor'] });
  const readLive = vi.fn((cursor: string | null) => Promise.resolve(cursor === null ? page([sibling], 'native-cursor') : page([restored, nested])));
  const live = new InfiniteQueryObserver(cache, { queryKey: key(false), initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => readLive(pageParam), getNextPageParam: (last) => last.next_cursor, staleTime: Infinity });
  releases.push(live.subscribe(() => undefined));
  const readGone = vi.fn(() => Promise.resolve({ pages: [page([])], pageParams: [null] }));
  watch(key(true), { pages: [page([before])], pageParams: [null] }, readGone);
  const personalKey = ['admin', 'orgs', { q: '', deleted: false, orgKind: 'personal' }] as const;
  const personal = vi.fn(() => Promise.resolve({ pages: [page([])], pageParams: [null] })); watch(personalKey, { pages: [page([])], pageParams: [null] }, personal);
  const unrelated = vi.fn(() => Promise.resolve([])); watch(['admin', 'users', {}], [], unrelated);
  const unopened = ['admin', 'orgs', { q: 'later', deleted: false }, { reader: 7, field: 'closed-form' }]; cache.setQueryData(unopened, { pages: [page([])], pageParams: [null] });
  await restore();
  expect(readLive.mock.calls).toEqual([[null], ['native-cursor']]); expect(readGone).toHaveBeenCalledOnce();
  expect(cache.getQueryData(key(false))).toEqual({ pages: [page([sibling], 'native-cursor'), page([restored, nested])], pageParams: [null, 'native-cursor'] });
  expect(cache.getQueryData(key(true))).toEqual({ pages: [page([])], pageParams: [null] }); expect(cache.getQueryData(unopened)).toBeUndefined();
  expect(personal).not.toHaveBeenCalled(); expect(unrelated).not.toHaveBeenCalled(); expect(cache.getQueryData(['node', restored.id])).toEqual(restored);
});

it('cancels a pre-restore page before the exact repair so its late response cannot repopulate Deleted', async () => {
  const old = deferred<{ pages: OrgPage[]; pageParams: null[] }>(), started = deferred<AbortSignal>();
  cache.setQueryData(key(true), { pages: [page([before])], pageParams: [null] }); let reads = 0;
  const observer = new QueryObserver(cache, { queryKey: key(true), staleTime: Infinity, queryFn: ({ signal }) => {
    reads += 1; if (reads === 1) { started.resolve(signal); return old.promise; }
    return Promise.resolve({ pages: [page([])], pageParams: [null] });
  } });
  releases.push(observer.subscribe(() => undefined)); const pending = observer.refetch(); const signal = await started.promise;
  await restore(); expect(signal.aborted).toBe(true); old.resolve({ pages: [page([before])], pageParams: [null] }); await pending;
  expect(reads).toBe(2); expect(cache.getQueryData(key(true))).toEqual({ pages: [page([])], pageParams: [null] });
});

it('stops the remaining exact projection work when the reader changes', async () => {
  const pending = deferred<{ pages: OrgPage[]; pageParams: null[] }>(), started = deferred<undefined>();
  watch(key(false), { pages: [page([])], pageParams: [null] }, () => { started.resolve(undefined); return pending.promise; });
  const gone = vi.fn(() => Promise.resolve({ pages: [page([])], pageParams: [null] })); watch(key(true), { pages: [page([before])], pageParams: [null] }, gone);
  const work = restore(); await started.promise;
  client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }); cache.clear();
  const replacement = { pages: [page([{ ...before, id: 'new-reader' }])], pageParams: [null] }; cache.setQueryData(key(true), replacement);
  pending.resolve({ pages: [page([restored])], pageParams: [null] }); await work;
  expect(gone).not.toHaveBeenCalled(); expect(cache.getQueryData(key(true))).toEqual(replacement);
});

it('keeps the returned system Agent and repairs only the observed bounded system list in native order', async () => {
  const created = { ...detailAgent, id: 'new-system-agent', home: null, owner: null, org_id: null }, earlier = { ...created, id: 'earlier', name: 'Earlier profile' };
  const read = vi.fn(() => Promise.resolve([earlier, created])); watch(['admin', 'agents'], [earlier], read);
  const unrelated = vi.fn(() => Promise.resolve([])); watch(['agents', 'other-org'], [], unrelated);
  await accepted({ method: 'POST', path: '/admin/agents', answer: created });
  expect(cache.getQueryData(['agent', created.id])).toEqual(created); expect(cache.getQueryData(['admin', 'agents'])).toEqual([earlier, created]);
  expect(read).toHaveBeenCalledOnce(); expect(unrelated).not.toHaveBeenCalled();
  await accepted({ method: 'POST', path: '/orgs/other-org/agents', answer: { ...detailAgent, id: 'org-agent' } });
  expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(['admin', 'agents'])).toEqual([earlier, created]);
});
