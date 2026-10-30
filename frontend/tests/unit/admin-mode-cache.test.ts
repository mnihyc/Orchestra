// @vitest-environment node
/** A change of session authority forgets the former view before reading its replacement. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct, followWith, userEventChanges } from '@/api/acts';
import * as management from '@/api/admin';
import { client } from '@/api/client';
import { settingReachQuery } from '@/api/settings-reach';
import { targetSettingsQuery } from '@/api/settings';
import { trailQuery, systemUsersQuery } from '@/api/trace';
import { nodeSettingsQuery } from '@/api/usage';
import { sourceQuery } from '@/features/access/changes';
import { FeedRegistry } from '@/realtime/sync';

it('registers the one mode write and routes session events through the same authority reset', () => {
  expect(findAct('PUT', '/me/admin-mode')?.changes).toEqual([{ session: true }]);
  expect(userEventChanges('admin_mode', {})).toEqual([{ session: true }]);
});

it('clears inactive answers and visible old data, rebases feeds, then reads the active view again', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let rebased = false;
  const session = vi.fn(() => { rebased = true; });
  followWith({ session, trees: () => undefined, thread: () => undefined, document: () => undefined, counters: () => undefined });
  const key = ['document', 'visible'];
  cache.setQueryData(key, ['private old answer']);
  cache.setQueryData(['document', 'inactive'], ['old inactive answer']);
  const read = vi.fn(() => { expect(rebased).toBe(true); return Promise.resolve(['ordinary current answer']); });
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const seen: (string[] | undefined)[] = [];
  const stop = observer.subscribe((value) => { seen.push(value.data); });
  await applyChanges([{ session: true }], cache);
  expect(session).toHaveBeenCalledOnce();
  expect(seen).toContain(undefined);
  expect(cache.getQueryData(key)).toEqual(['ordinary current answer']);
  expect(cache.getQueryData(['document', 'inactive'])).toBeUndefined();
  expect(read).toHaveBeenCalledOnce();
  stop(); cache.clear();
});

it('evicts the live projection before reloading from a full baseline', () => {
  const registry = new FeedRegistry();
  const seen: string[] = [];
  const leave = registry.use('org:team', () => ({ lastSeq: () => null, reload: () => { seen.push('load full'); }, refresh: () => undefined, stop: () => undefined }), () => { seen.push('forget old'); });
  registry.rebase();
  expect(seen).toEqual(['forget old', 'load full']);
  leave(); registry.clear();
});

it('cancels an old-authority read so its late private answer cannot refill the cache', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const read: { signal?: AbortSignal; finish?: (rows: string[]) => void } = {};
  const pending = cache.query({ queryKey: ['document', 'late'], queryFn: ({ signal }) => {
    read.signal = signal;
    return new Promise<string[]>((resolve) => { read.finish = resolve; });
  } }).catch(() => undefined);
  await applyChanges([{ session: true }], cache);
  expect(read.signal?.aborted).toBe(true);
  read.finish?.(['late private data']);
  await pending;
  expect(cache.getQueryData(['document', 'late'])).toBeUndefined();
  cache.clear();
});

const protectedReads = [
  ['system', management.systemQuery], ['installation', management.installationRecordsQuery],
  ['backups', management.backupRestoreListQuery],
  ['metrics', management.metricsQuery({ names: ['requests'], from: '2026-01-01', to: '2026-01-02' })],
  ['settings', management.systemSettingsQuery], ['users', management.usersQuery({ q: '', status: '', admins: false })],
  ['user search', management.userSearchQuery('reader')], ['orgs', management.adminOrgsQuery({ q: '', deleted: false })],
  ['owner choices', management.orgPeopleQuery('org', '')], ['hosts', management.hostsQuery],
  ['runtimes', management.runtimesQuery({ host: '', kind: '' })], ['capabilities', management.capabilitiesQuery],
  ['harness builds', management.harnessBuildsQuery], ['system agents', management.systemAgentsQuery],
  ['plugins', management.pluginsQuery], ['storage', management.storageQuery], ['pki', management.pkiQuery],
  ['audit', management.auditQuery({ actor: '', action: '', node: '', from: '', to: '' })],
  ['identity providers', management.identityProvidersQuery],
  ['setting reach', settingReachQuery({ kind: 'system' }, 'orgs.personal_enabled', 'current')],
  ['target settings', targetSettingsQuery({ kind: 'system' })], ['usage settings', nodeSettingsQuery(null)],
  ['system trail', trailQuery({ type: 'system', name: 'System' }, {})], ['trail people', systemUsersQuery('')],
  ['permission defaults', sourceQuery({ type: 'system-defaults', place: { kind: 'system', id: 'system', name: 'System' } })],
] as const;

it.each(protectedReads)('retires %s without rereading it when mode turns off, even with its observer still mounted', async (_name, options) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // This exercises the supplier's cache address/authority; the marker is not a native response fixture.
  const key = [...options.queryKey];
  cache.setQueryData(['admin-mode'], { available: true, as_admin: true });
  cache.setQueryData(key, 'Old privileged answer');
  const read = vi.fn(() => Promise.resolve('Forbidden reread'));
  const observer = new QueryObserver(cache, { queryKey: key, meta: options.meta, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  try {
    await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], cache);
    expect(read).not.toHaveBeenCalled();
    expect(cache.getQueryData(key)).toBeUndefined();
  } finally { stop(); cache.clear(); }
});

it('keeps the ordinary node versions of shared query suppliers eligible after mode loss', async () => {
  const node = { kind: 'node' as const, id: 'ordinary' };
  const queries = [targetSettingsQuery(node), nodeSettingsQuery(node.id), settingReachQuery(node, 'name', 'current'),
    trailQuery({ type: 'node', id: node.id, kind: 'project', orgId: 'org', name: 'Project', access: undefined }, {})];
  for (const options of queries) {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const key = [...options.queryKey];
    cache.setQueryData(key, 'Old ordinary answer');
    const read = vi.fn(() => Promise.resolve('Current ordinary answer'));
    const observer = new QueryObserver(cache, { queryKey: key, meta: options.meta, queryFn: read, staleTime: Infinity });
    const stop = observer.subscribe(() => undefined);
    try {
      await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], cache);
      expect(read).toHaveBeenCalledOnce();
      expect(cache.getQueryData(key)).toBe('Current ordinary answer');
    } finally { stop(); cache.clear(); }
  }
});

it('does not let an older paused authority reset reread after a newer reset finishes', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['document', 'ordinary'], 'Old document');
  const read = vi.fn(() => Promise.resolve('Current document'));
  const observer = new QueryObserver(cache, { queryKey: ['document', 'ordinary'], queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  let release!: () => void;
  const paused = new Promise<void>((resolve) => { release = resolve; });
  vi.spyOn(cache, 'cancelQueries').mockImplementationOnce(() => paused);
  try {
    const older = applyChanges([{ session: true, mode: { available: true, as_admin: false } }], cache);
    await applyChanges([{ session: true, mode: { available: true, as_admin: true } }], cache);
    const version = client.session.getState().readerVersion;
    release(); await older;
    expect(read).toHaveBeenCalledOnce();
    expect(cache.getQueryData(['admin-mode'])).toEqual({ available: true, as_admin: true });
    expect(client.session.getState().readerVersion).toBe(version);
  } finally { stop(); cache.clear(); }
});
