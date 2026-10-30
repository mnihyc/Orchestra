/** Membership-dependent HTTP projections have no native row in an authority reset.
 * Retake each visible owner once; hidden views expire without a request.
 */
import type { Query, QueryClient, QueryKey } from '@tanstack/react-query';
import { client } from '@/api/client';
import { queryFeed } from '@/realtime/queryFeeds';
import { fetchShellAuthority, meQuery, metaKey } from '@/api/auth';
import { adminModeQuery } from '@/api/adminMode';
import type { AdminMode, Me } from '@/generated/api-types';
import { orgsKey } from '@/api/nodes';
import { queryAuthorityAllows } from '@/api/queryAuthority';

/** Membership-dependent views without a complete native replacement row. */
const PEOPLE_PROJECTIONS = new Set([
  'members', 'holdings', 'suspensions', 'outsiders', 'outside-removal', 'invitations', 'people', 'people-search', 'holder-search',
  'chat-mentions', 'document-readers', 'org-groups', 'group-members', 'first-steps', 'page-people', 'roles', 'page',
  'page-effective', 'node', 'org', 'orgs', 'shared-with-me', 'chats', 'chat-states', 'user-shares', 'preview', 'document-related',
  'node-chat-opening',
]);

// These admin projections contain people, membership counts or member choices.
// System settings, runtime state and installation records have separate owners.
const ADMIN_PEOPLE_PROJECTIONS = new Set(['users', 'user-search', 'orgs', 'org-people']);

/** Also recognizes a legacy prefix dependency, so it cannot schedule a second repair. */
export function isPeopleProjection(key: QueryKey): boolean {
  if (key[0] === 'admin') return key.length === 1 || typeof key[1] === 'string' && ADMIN_PEOPLE_PROJECTIONS.has(key[1]);
  return typeof key[0] === 'string' && PEOPLE_PROJECTIONS.has(key[0]);
}

const generations = new WeakMap<QueryClient, number>();
const hintCuts = new WeakMap<QueryClient, number>();
interface NativePeopleRead { cut: number; reader: number | undefined; accepted: boolean }
const nativeReads = new WeakMap<Query, NativePeopleRead>();

/** A native hint follows its committed change; later native reads already check that change. */
export function capturePeopleHint(cache: QueryClient): number {
  const next = (hintCuts.get(cache) ?? 0) + 1;
  hintCuts.set(cache, next);
  return next;
}

/** Bind one supplier to the latest received hint, without using a clock or inferring permission. */
export function beginPeopleRead(cache: QueryClient, queryKey: QueryKey): { accept: () => void; cancel: () => void } {
  const query = cache.getQueryCache().find({ queryKey, exact: true });
  const read: NativePeopleRead = { cut: hintCuts.get(cache) ?? 0, reader: client.session.getState().readerVersion, accepted: false };
  if (query !== undefined) nativeReads.set(query, read);
  return {
    accept: () => { if (query !== undefined && nativeReads.get(query) === read) read.accepted = true; },
    cancel: () => { if (query !== undefined && nativeReads.get(query) === read) nativeReads.delete(query); },
  };
}

function checkedAfter(query: Query, cut: number | undefined): boolean {
  const read = nativeReads.get(query);
  return cut !== undefined && read !== undefined && read.cut >= cut && read.reader === client.session.getState().readerVersion
    && (query.state.fetchStatus === 'fetching' || read.accepted && query.state.status === 'success');
}

function modeSettled(cache: QueryClient): Promise<void> {
  if (cache.isMutating({ mutationKey: ['admin-mode'] }) === 0) return Promise.resolve();
  return new Promise((resolve) => {
    const leave = cache.getMutationCache().subscribe(() => {
      if (cache.isMutating({ mutationKey: ['admin-mode'] }) !== 0) return;
      leave(); resolve();
    });
  });
}

export async function repairPeople(cache: QueryClient, supplied: readonly QueryKey[] = [], cut?: number): Promise<void> {
  const version = client.session.getState().readerVersion;
  const generation = (generations.get(cache) ?? 0) + 1;
  generations.set(cache, generation);
  const current = () => client.session.getState().readerVersion === version && generations.get(cache) === generation;
  // A mode mutation owns its complete authority reset. If it succeeds that
  // newer reader supersedes this work; a refusal still needs the native hint.
  await modeSettled(cache);
  if (!current()) return;
  const queries = cache.getQueryCache().getAll().filter((query) =>
    isPeopleProjection(query.queryKey) && queryFeed(query.queryKey) === null
    && !checkedAfter(query, cut)
    && !supplied.some((key) => JSON.stringify(key) === JSON.stringify(query.queryKey)));
  await Promise.all(queries.map((query) => cache.cancelQueries({ queryKey: query.queryKey, exact: true })));
  // Cancellation yields too: a mode write started in that interval owns the
  // next reader just as one already pending when the hint arrived does.
  await modeSettled(cache);
  if (!current()) return;
  const reads = [];
  for (const query of queries) {
    if (cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query) continue;
    if (!query.isActive() || !queryAuthorityAllows(cache, query.meta)) {
      cache.removeQueries({ queryKey: query.queryKey, exact: true });
    } else {
      query.setState({ data: undefined, error: null, status: 'pending', fetchStatus: 'idle' });
      reads.push(query.queryKey);
    }
  }
  await Promise.all(reads.map((queryKey) => current() ? cache.refetchQueries({ queryKey, exact: true }) : Promise.resolve()));
}

interface AuthorityBaseline { person: Me; mode: AdminMode; changed: boolean; supplied: readonly QueryKey[] }
interface AuthorityFlight { version: number | undefined; done: Promise<AuthorityBaseline | null>; invalidate: () => void }
const authorityFlights = new WeakMap<QueryClient, AuthorityFlight>();

/** One current person/mode baseline precedes dependent reads after an authority hint.
 * The existing session-reset owner handles changed authority; a pending mode act
 * can supersede this hint, and an identical concurrent hint shares these reads.
 */
export function readAuthority(cache: QueryClient): Promise<AuthorityBaseline | null> {
  const version = client.session.getState().readerVersion;
  const previous = authorityFlights.get(cache);
  if (previous !== undefined && previous.version === version) {
    previous.invalidate();
    return previous.done;
  }
  const current = () => client.session.getState().readerVersion === version;
  // Hints before requests start share one cut. A later hint can describe a newer
  // authority revision, so it retires the captured read without waiting for it.
  let invalidate: (() => void) | undefined;
  const work = async (): Promise<AuthorityBaseline | null> => {
    for (;;) {
      await modeSettled(cache);
      if (!current()) return null;
      const beforePerson = cache.getQueryData<Me>(meQuery.queryKey);
      const beforeMode = cache.getQueryData<AdminMode>(adminModeQuery.queryKey);
      await Promise.all([meQuery.queryKey, adminModeQuery.queryKey].map((queryKey) => cache.cancelQueries({ queryKey, exact: true })));
      await modeSettled(cache);
      if (!current()) return null;
      const controller = new AbortController();
      let replaced = false;
      const superseded = (): boolean => replaced;
      invalidate = () => { replaced = true; controller.abort(); };
      const leave = client.session.subscribe(() => { if (!current()) controller.abort(); });
      let end!: () => void;
      const interrupted = new Promise<null>((resolve) => { end = () => resolve(null); });
      controller.signal.addEventListener('abort', end, { once: true });
      try {
        // Only this cut may publish. The race also retires suppliers which ignore abort.
        const values = await Promise.race([
          fetchShellAuthority(controller.signal),
          interrupted,
        ]);
        await modeSettled(cache);
        if (!current()) return null;
        if (superseded()) continue;
        if (values === null) return null;
        const { person, admin_mode: mode, orgs, meta } = values;
        const changed = beforePerson?.is_system_admin !== person.is_system_admin
          || beforeMode?.as_admin !== mode.as_admin || beforeMode.available !== mode.available;
        cache.setQueryData(adminModeQuery.queryKey, mode);
        if (!current()) return null;
        if (superseded()) continue;
        cache.setQueryData(meQuery.queryKey, person);
        if (superseded()) continue;
        if (!current()) return null;
        cache.setQueryData(orgsKey, orgs);
        cache.setQueryData(metaKey, meta);
        return { person, mode, changed, supplied: [orgsKey] };
      } catch (error) {
        if (!current()) return null;
        if (superseded()) continue;
        if (controller.signal.aborted) return null;
        throw error;
      } finally { invalidate = undefined; controller.signal.removeEventListener('abort', end); leave(); }
    }
  };
  const flight: AuthorityFlight = { version, done: work(), invalidate: () => invalidate?.() };
  authorityFlights.set(cache, flight);
  const clear = () => { if (authorityFlights.get(cache) === flight) authorityFlights.delete(cache); };
  void flight.done.then(clear, clear);
  return flight.done;
}
