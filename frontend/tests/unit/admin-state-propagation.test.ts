/** Native answers/hints repair exact observed projections, retaining filters, cursors and reader ownership. */
import { InfiniteQueryObserver, QueryClient, QueryObserver, type QueryKey } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct, userEventChanges } from '@/api/acts';
import { client, type Act } from '@/api/client';
import type { AdminUser, AdminUserPage } from '@/generated/api-types';
import { meta } from '../doubles/fixtures';

const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
const reader = client.session.getState();
const releases: (() => void)[] = [];
afterEach(() => { for (const release of releases.splice(0)) release(); cache.clear(); client.session.setState(reader); });
const person = (change: Partial<AdminUser> = {}): AdminUser => ({ id: 'person', username: 'person', display_name: 'A person', kind: 'human', avatar: { text: 'P', color: 1 },
  email: 'person@example.test', email_verified: true, status: 'active', is_system_admin: false, totp_enabled: false, email_code_second_step: false,
  locked: [], org_invited_at: null, created_at: '2026-10-01T00:00:00Z', last_seen_at: null, ...change });
const key = (status = '', q = '', admins = false) => ['admin', 'users', { q, status, admins }] as const;
const page = (...items: AdminUser[]): AdminUserPage => ({ items, next_cursor: null });
const pages = (...items: AdminUser[]) => ({ pages: [page(...items)], pageParams: [null] });
function observe<T>(queryKey: QueryKey, data: T, read: () => Promise<T>) {
  cache.setQueryData(queryKey, data);
  const observer = new QueryObserver(cache, { queryKey, queryFn: read, staleTime: Infinity });
  releases.push(observer.subscribe(() => undefined)); return observer;
}
async function accepted(act: Act) {
  const found = findAct(act.method, act.path, act);
  if (found === null) throw new Error('The act is unregistered');
  await applyChanges(found.changes, cache);
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve };
}

it('applies an AdminUser answer to known loaded pages and safe references without a read or private-field expansion', async () => {
  const before = person(), after = person({ status: 'suspended' }), read = vi.fn(() => Promise.resolve(pages(before)));
  const held = { pages: [{ items: [person({ id: 'earlier', username: 'alpha' })], next_cursor: 'alpha' }, page(before)], pageParams: [null, 'alpha'] };
  const list = vi.fn(() => Promise.resolve(held)), search = vi.fn(() => Promise.resolve(page(before)));
  observe(key(), held, list); observe(['admin', 'user-search', 'person'], page(before), search);
  observe(['admin', 'orgs', 'unrelated'], pages(before), read);
  cache.setQueryData(['trace-person', before.id], { id: before.id, username: before.username, display_name: before.display_name, avatar: before.avatar });
  await accepted({ method: 'PATCH', path: '/admin/users/person', answer: after });
  expect(cache.getQueryData(key())).toEqual({ ...held, pages: [held.pages[0], page(after)] });
  expect(cache.getQueryData(['admin', 'user-search', 'person'])).toEqual(page(after));
  expect(cache.getQueryData(['trace-person', before.id])).not.toHaveProperty('email');
  expect(cache.getQueryData(['trace-person', before.id])).not.toHaveProperty('status');
  expect(list).not.toHaveBeenCalled(); expect(search).not.toHaveBeenCalled(); expect(read).not.toHaveBeenCalled();
  expect(cache.getQueryData(['admin', 'orgs', 'unrelated'])).toEqual(pages(before));
});

it('lets the native exact filtered query decide membership and page boundaries with its original page parameters', async () => {
  const before = person({ display_name: '100% native name' }), after = person({ display_name: before.display_name, status: 'suspended' });
  const active = key('active', '100%'), suspended = key('suspended', '100%');
  const first = person({ id: 'first', username: 'alpha', display_name: '100% first' }), last = person({ id: 'last', username: 'zulu', display_name: '100% last' });
  cache.setQueryData(active, { pages: [{ items: [first], next_cursor: 'alpha' }, page(before)], pageParams: [null, 'alpha'] });
  const read = vi.fn((cursor: string | null): Promise<AdminUserPage> => Promise.resolve(cursor === null ? { items: [first], next_cursor: 'alpha' } : page(last)));
  const observer = new InfiniteQueryObserver(cache, { queryKey: active, initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => read(pageParam), getNextPageParam: (last) => last.next_cursor, staleTime: Infinity });
  releases.push(observer.subscribe(() => undefined));
  const entered = vi.fn(() => Promise.resolve(pages(after))); observe(suspended, pages(), entered);
  cache.setQueryData(key('suspended', 'inactive'), pages());
  await accepted({ method: 'PATCH', path: '/admin/users/person', answer: after });
  expect(read.mock.calls).toEqual([[null], ['alpha']]); expect(entered).toHaveBeenCalledOnce();
  expect(cache.getQueryData(active)).toEqual({ pages: [{ items: [first], next_cursor: 'alpha' }, page(last)], pageParams: [null, 'alpha'] });
  expect(cache.getQueryData(suspended)).toEqual(pages(after)); expect(cache.getQueryData(key('suspended', 'inactive'))).toBeUndefined();
});

it('repairs creation membership through the loaded native page without caching the password reset secret', async () => {
  const created = person(), read = vi.fn(() => Promise.resolve(pages(created))); observe(key(), pages(), read);
  await accepted({ method: 'POST', path: '/admin/users', answer: { user: created, reset_url: 'one-time-secret', expires_at: '2026-11-01T00:00:00Z' } });
  expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(key())).toEqual(pages(created));
  expect(JSON.stringify(cache.getQueryCache().getAll().map((query) => query.state.data))).not.toContain('one-time-secret');
});

it('repairs a 204 invitation from one observed page and distributes its native timestamp to other loaded copies', async () => {
  const before = person(), invited = person({ org_invited_at: '2026-10-18T12:34:56Z' });
  const read = vi.fn(() => Promise.resolve(pages(invited))), other = vi.fn(() => Promise.resolve(page(before)));
  observe(key(), pages(before), read); observe(['admin', 'user-search', 'person'], page(before), other);
  cache.setQueryData(key('', 'inactive'), pages(before));
  await accepted({ method: 'POST', path: '/admin/users/person/org-invitation' });
  expect(read).toHaveBeenCalledOnce(); expect(other).not.toHaveBeenCalled();
  expect(cache.getQueryData(key())).toEqual(pages(invited)); expect(cache.getQueryData(key('', 'inactive'))).toEqual(pages(invited));
  expect(cache.getQueryData(['admin', 'user-search', 'person'])).toEqual(page(invited));
  read.mockResolvedValue(pages(before)); await accepted({ method: 'DELETE', path: '/admin/users/person/org-invitation' });
  expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(['admin', 'user-search', 'person'])).toEqual(page(before));
});

it('does not load an unopened invitation target or a different admin projection', async () => {
  const unrelated = vi.fn(() => Promise.resolve(({ nodes: [] }))); observe(['admin', 'system'], { nodes: [] }, unrelated);
  cache.setQueryData(key(), pages(person()));
  await accepted({ method: 'POST', path: '/admin/users/person/org-invitation' });
  expect(cache.getQueryData(key())).toBeUndefined(); expect(unrelated).not.toHaveBeenCalled();
});

it('cancels an older admin page before applying the acknowledged row, so its late response cannot revert it', async () => {
  const pending = deferred<ReturnType<typeof pages>>(), started = deferred<AbortSignal>();
  cache.setQueryData(key(), pages(person()));
  const observer = new QueryObserver(cache, { queryKey: key(), staleTime: Infinity,
    queryFn: ({ signal }) => { started.resolve(signal); return pending.promise; } });
  releases.push(observer.subscribe(() => undefined)); const old = observer.refetch(); const signal = await started.promise;
  await accepted({ method: 'PATCH', path: '/admin/users/person', answer: person({ status: 'suspended' }) });
  expect(signal.aborted).toBe(true); pending.resolve(pages(person())); await old;
  expect(cache.getQueryData(key())).toEqual(pages(person({ status: 'suspended' })));
});

it('does not copy a late repaired invitation into a replacement reader', async () => {
  const pending = deferred<ReturnType<typeof pages>>(), started = deferred<undefined>();
  observe(key(), pages(person()), async () => { started.resolve(undefined); return pending.promise; });
  cache.setQueryData(['admin', 'user-search', 'person'], page(person()));
  const work = accepted({ method: 'POST', path: '/admin/users/person/org-invitation' }); await started.promise;
  client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }); cache.clear();
  const replacement = person({ id: 'new-reader' }); cache.setQueryData(['admin', 'user-search', 'person'], page(replacement));
  pending.resolve(pages(person({ org_invited_at: 'old-reader-time' }))); await work;
  expect(cache.getQueryData(['admin', 'user-search', 'person'])).toEqual(page(replacement));
});

it('uses a fresh native Meta after setting and clearing the banner, without rereading settings or guessing expired banners', async () => {
  const native = meta({ banner: null, may_create_org: false }), read = vi.fn(() => Promise.resolve(native));
  observe(['meta'], meta({ banner: { text: 'Old', level: 'warning', until: null } }), read);
  const settings = vi.fn(() => Promise.resolve([])); observe(['admin', 'settings'], [{ key: 'system.banner', value: 'old' }], settings);
  await accepted({ method: 'PUT', path: '/admin/settings/system.banner', answer: { key: 'system.banner', value: { text: 'Expired', level: 'warning', until: '2000-01-01T00:00:00Z' } } });
  expect(cache.getQueryData(['meta'])).toEqual(native); expect(read).toHaveBeenCalledOnce(); expect(settings).not.toHaveBeenCalled();
  await accepted({ method: 'DELETE', path: '/admin/settings/system.banner', answer: { key: 'system.banner', value: null } });
  expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(['admin', 'settings'])).toEqual([{ key: 'system.banner', value: null }]);
});

it('keeps unrelated system settings quiet and does not read inactive Meta', async () => {
  const read = vi.fn(() => Promise.resolve(meta())); observe(['meta'], meta(), read);
  await accepted({ method: 'PUT', path: '/admin/settings/hosts.full_percent', answer: { key: 'hosts.full_percent', value: 80 } });
  expect(read).not.toHaveBeenCalled(); for (const release of releases.splice(0)) release();
  await applyChanges(userEventChanges('org_creation', { what: 'org_creation' }), cache);
  expect(read).not.toHaveBeenCalled(); expect(cache.getQueryData(['meta'])).toBeUndefined();
});

it('repairs each payload-free hint as the current reader and keeps the initial connected notification quiet', async () => {
  const native = meta({ may_create_org: false }), read = vi.fn(() => Promise.resolve(native)); observe(['meta'], meta({ may_create_org: true }), read);
  expect(userEventChanges('connected', {})).toEqual([]); expect(userEventChanges('connected', { resumed: false })).toEqual([]);
  for (const what of ['org_creation', 'meta_changed']) await applyChanges(userEventChanges(what, { what, meta: { may_create_org: true } }), cache);
  expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(['meta'])).toEqual(native);
  await applyChanges(userEventChanges('connected', { resumed: true }), cache); expect(read).toHaveBeenCalledTimes(3);
});

it('retains an imperative Meta repair through observer handoff, then makes exactly one post-hint read', async () => {
  const first = deferred<ReturnType<typeof meta>>(), next = deferred<ReturnType<typeof meta>>(), fresh = deferred<undefined>();
  const signals: AbortSignal[] = []; const read = vi.fn(({ signal }: { signal: AbortSignal }) => {
    signals.push(signal); if (signals.length === 1) return first.promise; fresh.resolve(undefined); return next.promise;
  });
  const observer = new QueryObserver(cache, { queryKey: ['meta'], queryFn: read }); const release = observer.subscribe(() => undefined);
  const work = applyChanges([{ meta: true }], cache); release(); expect(signals[0]?.aborted).toBe(false);
  const shell = new QueryObserver(cache, { queryKey: ['meta'], queryFn: read }); releases.push(shell.subscribe(() => undefined));
  first.resolve(meta({ may_create_org: false })); await fresh.promise; next.resolve(meta({ may_create_org: true })); await work;
  expect(read).toHaveBeenCalledTimes(2); expect(signals.every((signal) => !signal.aborted)).toBe(true);
  expect(cache.getQueryData(['meta'])).toEqual(meta({ may_create_org: true }));
});

it('honors explicit Meta cancellation and reader replacement without an automatic followup', async () => {
  const pending = deferred<ReturnType<typeof meta>>(), read = vi.fn(async () => pending.promise);
  const observer = new QueryObserver(cache, { queryKey: ['meta'], queryFn: read }); releases.push(observer.subscribe(() => undefined));
  const work = applyChanges([{ meta: true }], cache);
  client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }); await cache.cancelQueries(); cache.clear();
  const replacement = meta({ may_create_org: false }); cache.setQueryData(['meta'], replacement); pending.resolve(meta({ may_create_org: true })); await work;
  expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(['meta'])).toEqual(replacement);
});

it('patches the returned identity provider and separately repairs its omitted public metadata', async () => {
  const before = { id: 'provider', key: 'sso', name: 'Sign in', enabled: true, revision: 1 }, after = { ...before, enabled: false, revision: 2 };
  const list = vi.fn(() => Promise.resolve([before])), metadata = vi.fn(() => Promise.resolve(meta({ identity_providers: [] })));
  observe(['admin', 'identity-providers'], [before], list); observe(['meta'], meta(), metadata);
  await accepted({ method: 'PATCH', path: '/admin/identity-providers/provider', answer: after });
  expect(cache.getQueryData(['admin', 'identity-providers'])).toEqual([after]); expect(list).not.toHaveBeenCalled(); expect(metadata).toHaveBeenCalledOnce();
  await accepted({ method: 'DELETE', path: '/admin/identity-providers/provider' }); expect(cache.getQueryData(['admin', 'identity-providers'])).toEqual([]);
});

it('applies the complete harness-build answer and reads only the observed distinct adapter projection', async () => {
  const before = [{ kind: 'adapter', version: '1', is_default: true }], after = [{ kind: 'adapter', version: '1', is_default: false }, { kind: 'adapter', version: '2', is_default: true }];
  const builds = vi.fn(() => Promise.resolve(before)), declared = [{ kind: 'adapter', title: 'Native adapter', builds: [{ version: '1', is_default: false }, { version: '2', is_default: true }], descriptor: { native: true } }];
  const kinds = vi.fn(() => Promise.resolve(declared)); observe(['admin', 'harnesses'], before, builds); observe<typeof declared>(['harness-kinds'], [], kinds);
  await accepted({ method: 'PUT', path: '/admin/harnesses/adapter/2', answer: after });
  expect(cache.getQueryData(['admin', 'harnesses'])).toEqual(after); expect(cache.getQueryData(['harness-kinds'])).toEqual(declared);
  expect(builds).not.toHaveBeenCalled(); expect(kinds).toHaveBeenCalledOnce();
});
