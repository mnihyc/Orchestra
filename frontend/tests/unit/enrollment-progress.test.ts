/** Enrollment discovery belongs to its issuer's receipt, then follows the native resource without polling. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, resourceChanges, userEventChanges } from '@/api/acts';
import { enrollmentProgressKey } from '@/api/enrollment';
import { resourceRows, type ResourceRows } from '@/api/listed';
import type { EnrollmentProgress, HiddenItem, Host, Machine } from '@/generated/api-types';
import { wantedResourceFeeds } from '@/realtime/resourceWants';
import { setResourceFollowing } from '@/realtime/resourceFollowing';
import { enrollmentHost, enrollmentMachine } from '../doubles/enrollment-row';
import { inventoryRow } from '../doubles/everything-row';

const RECEIPT = 'a'.repeat(64), OTHER_RECEIPT = 'b'.repeat(64);
const pending = (receipt = RECEIPT): EnrollmentProgress => ({ receipt, machine: null, host: null });
const progress = (machine: Machine, receipt = RECEIPT): EnrollmentProgress => ({ receipt, machine, host: null });
const event = (status: EnrollmentProgress) => userEventChanges('enrollment', { receipt: status.receipt, status });

it('discovers only the matching receipt and follows its row only while that progress is shown', async () => {
  const cache = new QueryClient(), key = enrollmentProgressKey(RECEIPT);
  cache.setQueryData(key, pending());
  cache.setQueryData(['machines'], []);
  const read = vi.fn(() => Promise.resolve(pending()));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    const unrelated = enrollmentMachine('other-dialog', true);
    await applyChanges(event(progress(unrelated, OTHER_RECEIPT)), cache);
    expect(cache.getQueryData(key)).toEqual(pending());
    expect(wantedResourceFeeds(cache)).toEqual(new Set());
    await applyChanges(userEventChanges('enrollment', { receipt: RECEIPT, status: progress(unrelated, OTHER_RECEIPT) }), cache);
    expect(cache.getQueryData(key)).toEqual(pending());

    const offline = enrollmentMachine('this-dialog', false);
    await applyChanges(event(progress(offline)), cache);
    expect(cache.getQueryData(key)).toEqual(progress(offline));
    expect(cache.getQueryData(['machines'])).toEqual([unrelated, offline]);
    expect(wantedResourceFeeds(cache)).toEqual(new Set(['machine:this-dialog']));
    const online = { ...offline, online: true };
    await applyChanges(resourceChanges('machine', online.id, { what: 'changed', row: { kind: 'machine', value: online } }), cache);
    expect(cache.getQueryData(key)).toEqual(progress(online));
    expect(read).not.toHaveBeenCalled();
    off();
    expect(wantedResourceFeeds(cache)).toEqual(new Set());
  } finally { off(); cache.clear(); }
});

it('adds a new Node only to its own loaded home list and keeps private-name metadata', async () => {
  const cache = new QueryClient();
  const hidden: HiddenItem = { id: 'private-node', name: 'Private node', kind: 'host', parent_id: null, owner: null, content_hidden: true, private: true };
  cache.setQueryData(['hosts', null], resourceRows<Host>([hidden]));
  cache.setQueryData(['hosts', 'home-a'], resourceRows<Host>([]));
  cache.setQueryData(['hosts', 'home-b'], resourceRows<Host>([]));
  const row = { ...enrollmentHost('new-node', false), org_id: 'home-a' };
  try {
    await applyChanges(event({ receipt: RECEIPT, machine: null, host: row }), cache);
    const all = cache.getQueryData<ResourceRows<Host>>(['hosts', null]);
    expect(all).toEqual(Object.assign([row], { privateRows: [hidden] }));
    expect(cache.getQueryData(['hosts', 'home-a'])).toEqual(Object.assign([row], { privateRows: [] }));
    expect(cache.getQueryData(['hosts', 'home-b'])).toEqual(Object.assign([], { privateRows: [] }));
    expect(cache.getQueryData(enrollmentProgressKey(RECEIPT))).toEqual({ receipt: RECEIPT, machine: null, host: row });
  } finally { cache.clear(); }
});

it('shows a newly enrolled machine through the exact native inventory cut, then applies online without another read', async () => {
  const cache = new QueryClient();
  const key = ['resource-list', 'reader', 'resources', { kinds: ['machine'], resource: { details: true } }];
  const offline = enrollmentMachine('new-inventory-member', false);
  const row = inventoryRow({ id: offline.id, name: offline.name, kind: 'machine', state: 'offline', resource: { kind: 'machine', value: offline } });
  const empty = { pages: [{ items: [], next_cursor: null }], pageParams: [null] };
  const page = { pages: [{ items: [row], next_cursor: null }], pageParams: [null] };
  cache.setQueryData(key, empty);
  const read = vi.fn(() => Promise.resolve(page));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    await applyChanges(event(progress(offline)), cache);
    expect(cache.getQueryData(key)).toEqual(page);
    expect(read).toHaveBeenCalledTimes(1);
    const online = { ...offline, online: true };
    await applyChanges(resourceChanges('machine', online.id, { what: 'changed', row: { kind: 'machine', value: online } }), cache);
    expect(cache.getQueryData(key)).toEqual({ ...page, pages: [{ ...page.pages[0], items: [{ ...row, state: 'online', resource: { kind: 'machine', value: online } }] }] });
    expect(read).toHaveBeenCalledTimes(1);
  } finally { off(); cache.clear(); }
});

it.each([
  [false, true, 'active', 'online'],
  [true, false, 'active', 'offline'],
  [true, true, 'revoked', 'revoked'],
  [true, true, 'active', 'online'],
] as const)('keeps machine inventory state and filters consistent on %s → %s (%s)', async (wasOnline, online, status, state) => {
  const cache = new QueryClient();
  const before = enrollmentMachine('live-state', wasOnline), after = { ...before, online, status };
  const previousState = wasOnline ? 'online' : 'offline';
  const row = inventoryRow({ id: before.id, name: before.name, kind: 'machine', state: previousState, resource: { kind: 'machine', value: before } });
  const page = { pages: [{ items: [row], next_cursor: null }], pageParams: [null] };
  const names = { ...page, pages: [{ ...page.pages[0], items: [{ ...row, resource: undefined }] }] };
  const key = ['resource-list', 'reader', 'resources', { kinds: ['machine'], resource: { details: true } }];
  const filtered = ['resource-list', 'reader', 'resources', { kinds: ['machine'], resource: { details: true, state: previousState } }];
  const nameKey = ['resource-navigation', 'reader', 'resources', { kinds: ['machine'], resource: { details: false } }];
  cache.setQueryData(['machine', before.id], before);
  cache.setQueryData(key, page); cache.setQueryData(filtered, page); cache.setQueryData(nameKey, names);
  const reads = vi.fn(() => Promise.resolve(page));
  const observers = [key, filtered, nameKey].map((queryKey) => new QueryObserver(cache, { queryKey, queryFn: reads, staleTime: Infinity }));
  const stops = observers.map((observer) => observer.subscribe(() => undefined));
  try {
    await applyChanges(resourceChanges('machine', after.id, { what: 'changed', row: { kind: 'machine', value: after } }), cache);
    expect(cache.getQueryData(key)).toEqual({ ...page, pages: [{ ...page.pages[0], items: [{ ...row, state, resource: { kind: 'machine', value: after } }] }] });
    expect(cache.getQueryData(nameKey)).toEqual({ ...names, pages: [{ ...names.pages[0], items: [{ ...row, state, resource: undefined }] }] });
    expect(cache.getQueryData(filtered)).toEqual({ ...page, pages: [{ ...page.pages[0], items: state === previousState
      ? [{ ...row, state, resource: { kind: 'machine', value: after } }] : [] }] });
    expect(cache.getQueryData(['machine', after.id])).toEqual(after);
    expect(reads).not.toHaveBeenCalled();
  } finally { for (const stop of stops) stop(); cache.clear(); }
});

it('does not let a delayed initial read replace the newer native enrollment', async () => {
  const cache = new QueryClient(), key = enrollmentProgressKey(RECEIPT);
  let finish: (value: EnrollmentProgress) => void = () => { throw new Error('progress not requested'); };
  const answer = new Promise<EnrollmentProgress>((resolve) => { finish = resolve; });
  const read = vi.fn(() => answer);
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    expect(read).toHaveBeenCalledTimes(1);
    const online = enrollmentMachine('just-connected', true);
    await applyChanges(event(progress(online)), cache);
    finish(pending()); await answer; await Promise.resolve();
    expect(cache.getQueryData(key)).toEqual(progress(online));
    expect(read).toHaveBeenCalledTimes(1);
  } finally { off(); cache.clear(); }
});

it.each(['machine', 'host'] as const)('keeps an acknowledged %s online when its older discovery notice arrives late', async (kind) => {
  const cache = new QueryClient(), key = enrollmentProgressKey(RECEIPT);
  const machine = enrollmentMachine('already-followed-machine', true), host = enrollmentHost('already-followed-host', true);
  const online: EnrollmentProgress = kind === 'machine' ? progress(machine) : { receipt: RECEIPT, machine: null, host };
  const offline: EnrollmentProgress = kind === 'machine' ? progress({ ...machine, online: false }) : { receipt: RECEIPT, machine: null, host: { ...host, machine: { ...host.machine, online: false } } };
  const id = kind === 'machine' ? machine.id : host.id;
  try {
    await applyChanges(event(online), cache);
    setResourceFollowing(kind, id, true);
    await applyChanges(event(offline), cache);
    expect(cache.getQueryData(key)).toEqual(online);
    expect(cache.getQueryData([kind, id])).toEqual(kind === 'machine' ? machine : host);
  } finally { setResourceFollowing(kind, id, false); cache.clear(); }
});

it('does not let a delayed reconnect read roll an online machine back to offline', async () => {
  const cache = new QueryClient(), key = enrollmentProgressKey(RECEIPT);
  const offline = enrollmentMachine('reconnected', false);
  cache.setQueryData(key, progress(offline));
  let finish: (value: EnrollmentProgress) => void = () => { throw new Error('progress not requested'); };
  const answer = new Promise<EnrollmentProgress>((resolve) => { finish = resolve; });
  const read = vi.fn(() => answer);
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    const reading = observer.refetch();
    expect(read).toHaveBeenCalledTimes(1);
    const online = { ...offline, online: true };
    await applyChanges(resourceChanges('machine', online.id, { what: 'changed', row: { kind: 'machine', value: online } }), cache);
    finish(progress(offline)); await answer; await reading;
    expect(cache.getQueryData(key)).toEqual(progress(online));
    expect(read).toHaveBeenCalledTimes(1);
  } finally { off(); cache.clear(); }
});

it('repairs a metadata-only notice through one exact visible receipt, never a list or a hidden receipt', async () => {
  const cache = new QueryClient(), key = enrollmentProgressKey(RECEIPT);
  const row = enrollmentMachine('recovered', true), reply = progress(row);
  cache.setQueryData(key, pending());
  cache.setQueryData(enrollmentProgressKey(OTHER_RECEIPT), pending(OTHER_RECEIPT));
  cache.setQueryData(['machines'], []);
  const read = vi.fn(() => Promise.resolve(reply)), readList = vi.fn(() => Promise.resolve([]));
  const one = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const list = new QueryObserver(cache, { queryKey: ['machines'], queryFn: readList, staleTime: Infinity });
  const stops = [one.subscribe(() => undefined), list.subscribe(() => undefined)];
  try {
    await applyChanges(userEventChanges('enrollment', { receipt: OTHER_RECEIPT }), cache);
    expect(cache.getQueryData(enrollmentProgressKey(OTHER_RECEIPT))).toBeUndefined();
    expect(read).not.toHaveBeenCalled();
    await applyChanges(userEventChanges('enrollment', { receipt: RECEIPT }), cache);
    expect(cache.getQueryData(key)).toEqual(reply);
    expect(read).toHaveBeenCalledTimes(1); expect(readList).not.toHaveBeenCalled();
    expect(wantedResourceFeeds(cache)).toEqual(new Set(['machine:recovered']));
  } finally { for (const off of stops) off(); cache.clear(); }
});
