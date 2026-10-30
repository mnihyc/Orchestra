// @vitest-environment node
/** A mode write owns its authority transition; directory repair never revives its former reader. */
import { setImmediate as nextTurn } from 'node:timers/promises';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges } from '@/api/acts';
import { client } from '@/api/client';
import { repairPeople } from '@/api/peopleChanges';
import { ADMIN_QUERY } from '@/api/queryAuthority';

function gate() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function setup() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['admin', 'org-people', 'team', ''];
  cache.setQueryData(['admin-mode'], { available: true, as_admin: true });
  cache.setQueryData(key, 'Previous directory');
  const read = vi.fn(() => Promise.resolve('Current directory'));
  const observer = new QueryObserver(cache, { ...ADMIN_QUERY, queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  return { cache, key, read, close: () => { stop(); cache.clear(); } };
}

it('yields to the confirmed mode reset without rereading the old privileged projection', async () => {
  const before = client.session.getState();
  const world = setup(); const pending = gate();
  const mutation = world.cache.getMutationCache().build(world.cache, { mutationKey: ['admin-mode'], mutationFn: () => pending.promise });
  const writing = mutation.execute(undefined);
  try {
    await nextTurn();
    const repair = repairPeople(world.cache);
    await nextTurn();
    expect(world.read).not.toHaveBeenCalled();
    await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], world.cache);
    pending.resolve(); await writing; await repair;
    expect(world.read).not.toHaveBeenCalled();
    expect(world.cache.getQueryData(world.key)).toBeUndefined();
  } finally { pending.resolve(); world.close(); client.session.setState(before); }
});

it('keeps a concurrent directory change when the pending mode write is refused', async () => {
  const world = setup(); const pending = gate();
  const mutation = world.cache.getMutationCache().build(world.cache, { mutationKey: ['admin-mode'], mutationFn: () => pending.promise });
  const writing = mutation.execute(undefined).catch(() => undefined);
  try {
    await nextTurn();
    const repair = repairPeople(world.cache);
    await nextTurn(); expect(world.read).not.toHaveBeenCalled();
    pending.reject(new Error('Refused')); await writing; await repair;
    expect(world.read).toHaveBeenCalledOnce();
    expect(world.cache.getQueryData(world.key)).toBe('Current directory');
  } finally { pending.resolve(); world.close(); }
});

it.each(['refused', 'confirmed'] as const)('rechecks mode ownership when a %s write begins while directory reads are being cancelled', async (outcome) => {
  const before = client.session.getState();
  const world = setup(); const pending = gate(); const cancelled = gate();
  const cancel = world.cache.cancelQueries.bind(world.cache);
  let writing: Promise<unknown> = Promise.resolve();
  vi.spyOn(world.cache, 'cancelQueries').mockImplementationOnce(async (filters, options) => {
    await cancel(filters, options);
    const mutation = world.cache.getMutationCache().build(world.cache, { mutationKey: ['admin-mode'], mutationFn: () => pending.promise });
    writing = mutation.execute(undefined).catch(() => undefined);
    cancelled.resolve();
  });
  try {
    const repair = repairPeople(world.cache);
    await cancelled.promise; await nextTurn();
    expect(world.read).not.toHaveBeenCalled();
    if (outcome === 'confirmed') {
      await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], world.cache);
      pending.resolve(); await writing; await repair;
      expect(world.read).not.toHaveBeenCalled();
      expect(world.cache.getQueryData(world.key)).toBeUndefined();
    } else {
      pending.reject(new Error('Refused')); await writing; await repair;
      expect(world.read).toHaveBeenCalledOnce();
      expect(world.cache.getQueryData(world.key)).toBe('Current directory');
    }
  } finally { pending.resolve(); vi.restoreAllMocks(); world.close(); client.session.setState(before); }
});

it('repairs only directory-owned admin views and coalesces concurrent hints before reading', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['admin-mode'], { available: true, as_admin: true });
  const relevant = ['users', 'user-search', 'orgs', 'org-people'];
  const unrelated = ['system', 'installation-records', 'backup-restore-list', 'metrics', 'settings', 'hosts', 'runtimes', 'capabilities', 'harnesses', 'agents', 'plugins', 'storage', 'pki', 'audit', 'identity-providers'];
  const queries = [...relevant, ...unrelated].map((name) => {
    const key = ['admin', name];
    cache.setQueryData(key, 'Previous');
    const read = vi.fn(() => Promise.resolve('Current'));
    const observer = new QueryObserver(cache, { ...ADMIN_QUERY, queryKey: key, queryFn: read, staleTime: Infinity });
    return { name, key, read, stop: observer.subscribe(() => undefined) };
  });
  cache.setQueryData(['admin', 'settings', 'hidden'], 'Hidden settings');
  try {
    await Promise.all([applyChanges([{ people: true }], cache), applyChanges([{ people: true }], cache)]);
    for (const query of queries) {
      expect(query.read).toHaveBeenCalledTimes(relevant.includes(query.name) ? 1 : 0);
      expect(cache.getQueryData(query.key)).toBe(relevant.includes(query.name) ? 'Current' : 'Previous');
    }
    expect(cache.getQueryData(['admin', 'settings', 'hidden'])).toBe('Hidden settings');
  } finally { queries.forEach((query) => query.stop()); cache.clear(); }
});

it('retires every privileged supplier when the session owner confirms mode off', async () => {
  const before = client.session.getState();
  const world = setup();
  const key = ['admin', 'settings'];
  world.cache.setQueryData(key, 'Privileged settings');
  const read = vi.fn(() => Promise.resolve('Should not be read'));
  const observer = new QueryObserver(world.cache, { ...ADMIN_QUERY, queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  try {
    await applyChanges([{ session: true, mode: { available: false, as_admin: false } }], world.cache);
    expect(world.cache.getQueryData(world.key)).toBeUndefined();
    expect(world.cache.getQueryData(key)).toBeUndefined();
    expect(world.read).not.toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); world.close(); client.session.setState(before); }
});

it('does not treat an unknown session mode as authority for a protected directory supplier', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['admin', 'org-people', 'team', ''];
  cache.setQueryData(key, 'Previous directory');
  const read = vi.fn(() => Promise.resolve('Should not be read'));
  const stop = new QueryObserver(cache, { ...ADMIN_QUERY, queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges([{ people: true }], cache);
    expect(read).not.toHaveBeenCalled();
    expect(cache.getQueryData(key)).toBeUndefined();
  } finally { stop(); cache.clear(); }
});
