// @vitest-environment node
/** Native answers and scoped live rows keep former refresh consumers coherent without refetching lists. */
import { QueryClient, QueryObserver, type QueryKey } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { applyChanges, findAct, followWith, resourceChanges, userEventChanges } from '@/api/acts';
import { ApiError, type Act } from '@/api/client';
import { namesKey } from '@/api/search';
import type { NameHit, ResolvedSetting, Thread } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

let cache: QueryClient;
let stops: (() => void)[];
beforeEach(() => { cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); stops = []; });
afterEach(() => {
  for (const stop of stops) stop();
  cache.clear();
  followWith({ thread: () => undefined, trees: () => undefined, document: () => undefined, counters: () => undefined });
});

async function watch(queryKey: QueryKey, initial: unknown = 0) {
  const queryFn = vi.fn(() => Promise.resolve(initial));
  await cache.query({ queryKey, queryFn });
  const observer = new QueryObserver(cache, { queryKey, queryFn, staleTime: Infinity });
  stops.push(observer.subscribe(() => undefined));
  return queryFn;
}

async function acted(method: string, path: string, payload: Pick<Act, 'input' | 'answer'> = {}) {
  const entry = findAct(method, path, payload);
  expect(entry).not.toBeNull();
  await applyChanges(entry?.changes ?? [], cache);
}

function setting(key: string, value: unknown): ResolvedSetting {
  return { key, value, own: null, source: { scope: 'system', node: null }, locked_by: null, writable: true, parts: [] };
}

const visited: NameHit = { id: 'topic-one', kind: 'topic', name: 'Fix login', path: [], recent: true, archived: false,
  address: null, avatar: null, owner: null, username: null, color: null, custom_color: null, tag: null, thread_attention: null };

describe('the registry replaces the last screen refreshes', () => {
  it('a visit starts no lookup, while native recents update without changing the held empty or typed finder', async () => {
    const recent = ['recents'];
    const empty = namesKey({ kinds: ['org', 'topic'], q: '' });
    const typed = namesKey({ kinds: ['org', 'topic'], q: 'fix' });
    const emptyPage = { items: [], partial: false };
    const typedPage = { items: [visited], partial: false };
    const recents = await watch(recent, []); const finder = await watch(empty, emptyPage); const searching = await watch(typed, typedPage);
    await acted('POST', '/me/recents', { input: { node_id: visited.id } });
    await acted('POST', '/me/visits', { input: { kind: visited.kind, id: visited.id } });
    expect(cache.getQueryData(recent)).toEqual([]);
    expect(cache.getQueryData(empty)).toEqual(emptyPage);
    expect(cache.getQueryData(typed)).toEqual(typedPage);
    await applyChanges(userEventChanges('recents', { recents: [visited] }), cache);
    expect(cache.getQueryData(recent)).toEqual([visited]);
    expect(cache.getQueryData(empty)).toEqual(emptyPage);
    expect(cache.getQueryData(typed)).toEqual(typedPage);
    expect(recents).toHaveBeenCalledTimes(1);
    expect(finder).toHaveBeenCalledTimes(1);
    expect(searching).toHaveBeenCalledTimes(1);
  });

  it.each(['PUT', 'DELETE'])('%s of a global host policy applies its setting answer and the native host row to both views without refetching', async (method) => {
    const initial = method === 'PUT' ? 'warn' : 'block';
    const next = method === 'PUT' ? 'block' : 'warn';
    const before = { id: 'one', name: 'Workshop', org_id: 'org', when_full: { value: initial, own: false }, revision: 1, access: { permissions: ['host.view'] } };
    const unrelated = { ...before, id: 'two', name: 'Elsewhere' };
    const beforeSetting = { ...setting('hosts.when_full', initial), own: { value: initial, revision: 1, locked: false } };
    const changedSetting = { ...setting('hosts.when_full', next), own: method === 'PUT' ? { value: next, revision: 2, locked: false } : null };
    const detail = await watch(['host', 'one'], before); const list = await watch(['hosts', 'org'], [before]);
    const other = await watch(['host', 'two'], unrelated); const settings = await watch(['admin', 'settings'], [beforeSetting]);
    const model = setting('agent.model', method === 'PUT' ? 'local-model' : null);
    await acted(method, '/admin/settings/agent.model', { answer: model });
    expect(cache.getQueryData(['host', 'one'])).toEqual(before);
    expect(cache.getQueryData(['hosts', 'org'])).toEqual([before]);
    await acted(method, '/admin/settings/hosts.when_full', { answer: changedSetting });
    expect(cache.getQueryData(['admin', 'settings'])).toEqual([changedSetting, model]);
    // A setting is not a guessed host DTO. Its scoped feed supplies the resolved native view.
    expect(cache.getQueryData(['host', 'one'])).toEqual(before);
    const after = { ...before, when_full: { value: next, own: false }, revision: 2 };
    await applyChanges(resourceChanges('host', after.id, { host: after }), cache);
    expect(cache.getQueryData(['host', 'one'])).toEqual(after);
    expect(cache.getQueryData(['hosts', 'org'])).toEqual([after]);
    expect(cache.getQueryData(['host', 'two'])).toEqual(unrelated);
    for (const read of [detail, list, other, settings]) expect(read).toHaveBeenCalledTimes(1);
  });

  it('closing a side removes its open row without refetching the parent lists, recorded history or removed node', async () => {
    const open = ['thread-sides', 'parent', 'open'];
    const closed = ['thread-sides', 'parent', 'closed'];
    const side = { id: 'side', kind: 'thread', name: 'Check the release', side_of: { id: 'parent' }, access: { permissions: ['thread.view'] } };
    const peer = { ...side, id: 'peer', name: 'Review the tests' };
    const recorded = { ...side, id: 'recorded', name: 'Earlier review' };
    const opened = await watch(open, [side, peer]); const history = await watch(closed, [recorded]);
    const removed = await watch(['node', 'side'], side);
    await acted('DELETE', '/nodes/side');
    expect(cache.getQueryData(open)).toEqual([peer]);
    expect(cache.getQueryData(closed)).toEqual([recorded]);
    expect(cache.getQueryData(['node', 'side'])).toBeUndefined();
    expect(cache.getQueryState(['node', 'side'])?.error).toBeInstanceOf(ApiError);
    expect(cache.getQueryState(['node', 'side'])?.error).toMatchObject({ problem: { code: 'not_found' } });
    expect(opened).toHaveBeenCalledTimes(1);
    expect(history).toHaveBeenCalledTimes(1);
    expect(removed).toHaveBeenCalledTimes(1);
  });

  it('a preference notice without a value does not refetch setup facts or saved state', async () => {
    const preferences = { help: { tutorials: { one: 'done' } }, sidebar: { folded: ['resources'] } };
    const facts = await watch(['first-steps', 'org']); const state = await watch(['me-state'], preferences);
    await acted('PUT', '/me/state/sidebar');
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    await acted('PUT', '/me/state/help');
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    expect(cache.getQueryData(['me-state'])).toEqual(preferences);
    await applyChanges(userEventChanges('state', { key: 'help' }), cache);
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    await applyChanges(userEventChanges('connected', {}), cache);
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    expect(cache.getQueryData(['me-state'])).toEqual(preferences);
    // Absence is not a clear. An explicitly returned null still clears exactly that preference.
    await acted('PUT', '/me/state/help', { answer: null });
    expect(cache.getQueryData(['me-state'])).toEqual({ ...preferences, help: null });
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    expect(facts).toHaveBeenCalledTimes(1);
    expect(state).toHaveBeenCalledTimes(1);
  });

  it('the atomic setup choice applies its answer without rereading facts or preferences', async () => {
    const facts = await watch(['first-steps', 'org']); const state = await watch(['me-state']);
    const saved = { help: { first_steps: { org_id: 'org', work_mode: 'team', dismissed: true } }, sidebar: { last_org_id: 'org' } };
    const act = findAct('PUT', '/me/first-steps', { answer: saved });
    expect(act).not.toBeNull();
    await applyChanges(act?.changes ?? [], cache);
    expect(cache.getQueryData(['me-state'])).toEqual(saved);
    expect(cache.getQueryData(['first-steps', 'org'])).toBe(0);
    expect(facts).toHaveBeenCalledTimes(1);
    expect(state).toHaveBeenCalledTimes(1);
  });

  it('another window receives only the changed setup fields and keeps its other portable state', async () => {
    const state = await watch(['me-state']);
    cache.setQueryData(['me-state'], { help: { tutorials: { one: 'done' } }, sidebar: { folded: ['resources'] } });
    const choice = { org_id: 'org', work_mode: 'team', dismissed: true };
    await applyChanges(userEventChanges('state', { key: 'help', child: 'first_steps', value: choice }), cache);
    await applyChanges(userEventChanges('state', { key: 'sidebar', child: 'last_org_id', value: 'org' }), cache);
    expect(cache.getQueryData(['me-state'])).toEqual({ help: { tutorials: { one: 'done' }, first_steps: choice }, sidebar: { folded: ['resources'], last_org_id: 'org' } });
    expect(state).toHaveBeenCalledTimes(1);
  });

  it('a node rename applies its native name to held projections without retaking the thread feed', async () => {
    const thread = vi.fn();
    followWith({ thread, trees: () => undefined, document: () => undefined, counters: () => undefined });
    const world = new World();
    const person = world.people.get('ada');
    const source = world.places.rowOf(ACME.mainThread);
    const otherSource = world.places.rowOf(ACME.depsThread);
    if (person === undefined || source === undefined || otherSource === undefined) throw new Error('the native world supplies both readable threads');
    const before = world.threads.thread(person, source);
    const unrelated = world.threads.thread(person, otherSource);
    const after: Thread = { ...before, name: 'Reviewed title', revision: before.revision + 1,
      path: before.path.map((part) => part.id === before.id ? { ...part, name: 'Reviewed title' } : part) };
    const node = await watch(['node', before.id], before); const conversation = await watch(['thread', before.id], before);
    const names = await watch(['recents'], [{ id: before.id, kind: before.kind, name: before.name }]);
    const other = await watch(['node', unrelated.id], unrelated);
    await acted('PATCH', `/nodes/${before.id}`, { input: { name: after.name }, answer: after });
    expect(cache.getQueryData(['node', before.id])).toEqual(after);
    expect(cache.getQueryData(['thread', before.id])).toEqual(after);
    expect(cache.getQueryData(['recents'])).toEqual([{ id: before.id, kind: before.kind, name: after.name }]);
    expect(cache.getQueryData(['node', unrelated.id])).toEqual(unrelated);
    expect(thread).not.toHaveBeenCalled();
    for (const read of [node, conversation, names, other]) expect(read).toHaveBeenCalledTimes(1);
  });
});
