// @vitest-environment node
/** An authority hint refreshes the person and session before dependent views read again. */
import { setImmediate as nextTurn } from 'node:timers/promises';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyChanges, type Change } from '@/api/acts';
import { client } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import type { AdminMode, Me } from '@/generated/api-types';
import { me, meta } from '../doubles/fixtures';
import type * as AuthApi from '@/api/auth';
import type * as AdminModeApi from '@/api/adminMode';

const reads = vi.hoisted(() => ({ person: vi.fn<() => Promise<Me>>(), mode: vi.fn<() => Promise<AdminMode>>() }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['me'], queryFn: reads.person, staleTime: Infinity },
  fetchShellAuthority: () => Promise.all([reads.person(), reads.mode()]).then(([person, admin_mode]) => ({ person, admin_mode, orgs: [], meta: meta() })),
}));
vi.mock('@/api/adminMode', async (original) => ({ ...await original<typeof AdminModeApi>(), adminModeQuery: { queryKey: ['admin-mode'], queryFn: reads.mode, staleTime: Infinity } }));
const reader = client.session.getState();
const mode = { available: true, as_admin: true };
const authority: Change & { authority: true } = { people: true, authority: true };
beforeEach(() => { reads.person.mockResolvedValue(me(1, 'person', 'Person', { is_system_admin: true })); reads.mode.mockResolvedValue(mode); });
afterEach(() => { vi.clearAllMocks(); client.session.setState(reader); });

function gate() {
  let resolve!: () => void, reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function world() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['me'], me(1, 'person', 'Person', { is_system_admin: true }));
  cache.setQueryData(['admin-mode'], mode);
  const entries = [
    { key: ['members', 'team'], meta: {}, read: vi.fn(() => Promise.resolve('Current members')) },
    { key: ['admin', 'settings'], ...ADMIN_QUERY, read: vi.fn(() => Promise.resolve('System settings')) },
    { key: ['org-admin', 'team'], meta: { requiresAdminMode: true }, read: vi.fn(() => Promise.resolve('Org settings')) },
  ];
  const stops = entries.map((entry) => {
    cache.setQueryData(entry.key, 'Previous');
    return new QueryObserver(cache, { queryKey: entry.key, meta: entry.meta, queryFn: entry.read, staleTime: Infinity }).subscribe(() => undefined);
  });
  return { cache, entries, close: () => { stops.forEach((stop) => stop()); cache.clear(); } };
}

it('retires global-admin suppliers after demotion while preserving the actual org-admin mode', async () => {
  const held = world();
  reads.person.mockResolvedValue(me(1, 'person', 'Person', { is_system_admin: false }));
  try {
    await applyChanges([authority], held.cache);
    expect(held.cache.getQueryData<Me>(['me'])?.is_system_admin).toBe(false);
    expect(held.cache.getQueryData(['admin-mode'])).toEqual(mode);
    expect(held.cache.getQueryData(['admin', 'settings'])).toBeUndefined();
    expect(held.entries[1]?.read).not.toHaveBeenCalled();
    expect(held.entries[2]?.read).toHaveBeenCalledOnce();
    expect(held.cache.getQueryData(['org-admin', 'team'])).toBe('Org settings');
    expect(reads.person).toHaveBeenCalledOnce(); expect(reads.mode).toHaveBeenCalledOnce();
  } finally { held.close(); }
});

it('shares concurrent authority baselines and keeps unrelated admin views quiet for membership changes', async () => {
  const held = world();
  try {
    await Promise.all([applyChanges([authority], held.cache), applyChanges([authority], held.cache)]);
    expect(reads.person).toHaveBeenCalledOnce(); expect(reads.mode).toHaveBeenCalledOnce();
    expect(held.entries[0]?.read).toHaveBeenCalledOnce();
    expect(held.entries[1]?.read).not.toHaveBeenCalled();
    expect(held.entries[2]?.read).not.toHaveBeenCalled();
    expect(held.cache.getQueryData(['admin', 'settings'])).toBe('Previous');
  } finally { held.close(); }
});

it.each(['confirmed', 'refused'] as const)('yields to a pending mode owner that is %s before asking the authority baseline', async (outcome) => {
  const held = world(), pending = gate();
  const mutation = held.cache.getMutationCache().build(held.cache, { mutationKey: ['admin-mode'], mutationFn: () => pending.promise });
  const writing = mutation.execute(undefined).catch(() => undefined);
  try {
    await nextTurn();
    const updating = applyChanges([authority], held.cache);
    await nextTurn();
    expect(reads.person).not.toHaveBeenCalled(); expect(reads.mode).not.toHaveBeenCalled();
    if (outcome === 'confirmed') {
      await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], held.cache);
      pending.resolve(); await writing; await updating;
      expect(reads.person).not.toHaveBeenCalled(); expect(reads.mode).not.toHaveBeenCalled();
      expect(held.entries[1]?.read).not.toHaveBeenCalled();
    } else {
      pending.reject(new Error('Refused')); await writing; await updating;
      expect(reads.person).toHaveBeenCalledOnce(); expect(reads.mode).toHaveBeenCalledOnce();
      expect(held.entries[0]?.read).toHaveBeenCalledOnce();
      expect(held.entries[1]?.read).not.toHaveBeenCalled();
    }
  } finally { pending.resolve(); held.close(); }
});

it('does not publish either late capsule response into the reader confirmed by a mode write', async () => {
  const held = world(), pending = gate();
  let personDone!: (person: Me) => void, modeDone!: (mode: AdminMode) => void;
  const oldPerson = me(1, 'person', 'Old person', { is_system_admin: true });
  const currentPerson = me(1, 'person', 'Current person', { is_system_admin: false });
  reads.person.mockReturnValue(new Promise<Me>((resolve) => { personDone = resolve; }));
  reads.mode.mockReturnValue(new Promise<AdminMode>((resolve) => { modeDone = resolve; }));
  let writing: Promise<unknown> = Promise.resolve();
  try {
    const updating = applyChanges([authority], held.cache);
    await nextTurn();
    expect(reads.person).toHaveBeenCalledOnce(); expect(reads.mode).toHaveBeenCalledOnce();
    const mutation = held.cache.getMutationCache().build(held.cache, { mutationKey: ['admin-mode'], mutationFn: () => pending.promise });
    writing = mutation.execute(undefined).catch(() => undefined);
    await nextTurn();
    await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], held.cache);
    held.cache.setQueryData(['me'], currentPerson);
    pending.resolve(); await writing;
    // Deliberately ignore abort, as an already-received HTTP response can do.
    personDone(oldPerson); modeDone(mode); await updating;
    expect(held.cache.getQueryData(['admin-mode'])).toEqual({ available: true, as_admin: false });
    expect(held.cache.getQueryData(['me'])).toEqual(currentPerson);
    expect(held.entries[1]?.read).not.toHaveBeenCalled();
  } finally { pending.resolve(); personDone(oldPerson); modeDone(mode); await writing; held.close(); }
});

it('replaces a started authority baseline when a later hint arrives and never publishes its late old standing', async () => {
  const held = world();
  let oldPersonDone!: (person: Me) => void, oldModeDone!: (mode: AdminMode) => void;
  const oldPerson = me(1, 'person', 'Former administrator', { is_system_admin: true });
  const demoted = me(1, 'person', 'Current organization administrator', { is_system_admin: false });
  const oldPersonRead = new Promise<Me>((resolve) => { oldPersonDone = resolve; });
  const oldModeRead = new Promise<AdminMode>((resolve) => { oldModeDone = resolve; });
  reads.person.mockImplementation(() => reads.person.mock.calls.length === 1 ? oldPersonRead : Promise.resolve(demoted));
  reads.mode.mockImplementation(() => reads.mode.mock.calls.length === 1 ? oldModeRead : Promise.resolve(mode));
  const first = applyChanges([authority], held.cache);
  let later: Promise<void> = Promise.resolve();
  try {
    await nextTurn();
    expect(reads.person).toHaveBeenCalledOnce(); expect(reads.mode).toHaveBeenCalledOnce();
    // A later settled native hint can name a different authority cut while the reader is unchanged.
    later = applyChanges([authority], held.cache);
    await nextTurn();
    expect(reads.person).toHaveBeenCalledTimes(2); expect(reads.mode).toHaveBeenCalledTimes(2);
    await Promise.all([first, later]);
    expect(held.cache.getQueryData(['me'])).toEqual(demoted);
    expect(held.cache.getQueryData(['admin-mode'])).toEqual(mode);
    expect(held.cache.getQueryData(['admin', 'settings'])).toBeUndefined();
    expect(held.entries[1]?.read).not.toHaveBeenCalled();
    // Already-dispatched suppliers may ignore abort; the replacement cannot wait for or accept them.
    oldPersonDone(oldPerson); oldModeDone(mode); await nextTurn();
    expect(held.cache.getQueryData(['me'])).toEqual(demoted);
    expect(held.cache.getQueryData(['admin', 'settings'])).toBeUndefined();
    expect(reads.person).toHaveBeenCalledTimes(2); expect(reads.mode).toHaveBeenCalledTimes(2);
  } finally { oldPersonDone(oldPerson); oldModeDone(mode); await Promise.allSettled([first, later]); held.close(); }
});
