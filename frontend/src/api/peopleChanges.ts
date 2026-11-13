/** Membership-dependent HTTP projections have no native row in an authority reset.
 * Retake each visible owner once; hidden views expire without a request.
 */
import type { Query, QueryClient, QueryKey } from '@tanstack/react-query';
import { ApiError, client } from '@/api/client';
import { hasDefaultsOpeningOwner } from '@/api/defaultsOpening';
import { queryFeed } from '@/realtime/queryFeeds';
import { fetchShellAuthority, meQuery, metaKey } from '@/api/auth';
import { adminModeQuery } from '@/api/adminMode';
import type { AdminMode, Me } from '@/generated/api-types';
import { orgsKey } from '@/api/nodes';
import { queryAuthorityAllows } from '@/api/queryAuthority';
import { scheduledReads } from '@/api/readScheduler';

/** Membership-dependent views without a complete native replacement row. */
const PEOPLE_PROJECTIONS = new Set([
  'members', 'holdings', 'suspensions', 'outsiders', 'outside-removal', 'invitations', 'people', 'people-search', 'holder-search',
  'chat-mentions', 'document-readers', 'org-groups', 'group-members', 'first-steps', 'page-people', 'roles', 'page',
  'page-effective', 'node', 'org', 'orgs', 'shared-with-me', 'chats', 'chat-states', 'user-shares', 'preview', 'document-related',
  'node-chat-opening', 'org-overview', 'children', 'names', 'activity', 'outside-port-openings',
  // A private reference is an independent authorized projection; a reconnect can change its reach.
  'reference-preview',
]);

// These admin projections contain people, membership counts or member choices.
// System settings, runtime state and installation records have separate owners.
const ADMIN_PEOPLE_PROJECTIONS = new Set(['users', 'user-search', 'orgs', 'org-people']);

/** Also recognizes a legacy prefix dependency, so it cannot schedule a second repair. */
export function isPeopleProjection(key: QueryKey): boolean {
  if (key[0] === 'away') return key[1] === 'outside-status';
  if (key[0] === 'usage') return key[1] === 'figures';
  if (key[0] === 'admin') return key.length === 1 || typeof key[1] === 'string' && ADMIN_PEOPLE_PROJECTIONS.has(key[1]);
  return typeof key[0] === 'string' && PEOPLE_PROJECTIONS.has(key[0]);
}

const generations = new WeakMap<QueryClient, number>();
const hintCuts = new WeakMap<QueryClient, number>();
interface NativePeopleRead { cut: number; reader: number | undefined; accepted: boolean }
const nativeReads = new WeakMap<Query, NativePeopleRead>();

/** A native hint follows its committed change; later native reads already check that change. */
export function capturePeopleHint(cache: QueryClient): number {
  scheduledReads.advance();
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

/** Current native membership generation; attempt history remains with its own Query. */
export function peopleHintCut(cache: QueryClient): number { return hintCuts.get(cache) ?? 0; }

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

/** What a re-read that refuses says: the person may no longer see this, or it is gone. */
function refuses(error: unknown): boolean {
  return error instanceof ApiError && (error.status === 401 || error.status === 403 || error.status === 404 || error.status === 410);
}

/** The records of what a person may do with a place, which the screens built on them hide, with their state, until they are read again. */
const REVEALING = new Set(['org']);

/**
 * Reads again every view of a person's reach that is on screen, once: after a reconnect, which has said nothing about reach,
 * and after an act whose answer says that reach changed (`reachChanged`).
 *
 * A view keeps showing what it has while it is read again. It gives it up at once when the re-read refuses it (it is no
 * longer the person's to see) and replaces it when the re-read returns something else; a re-read that fails for the network
 * or the server leaves it as it was, with the failure told as any failed read is. The screen showed that data until this
 * moment, and the re-read is the first at which anything can be learnt, so emptying it first would buy no privacy: it would
 * make every screen draw its loading view, and give up what its children hold (a typed text, a place in a list, the keyboard,
 * a dialog), for the length of the re-read.
 *
 * One record is the exception when an act's answer has put reach in doubt: the person's access to an org (`REVEALING`) is
 * emptied first, as it always has been, because the org screens hide what the person may do with the org, and keep their
 * state, until it is read again (tests/component/org-authority-intent.test.tsx). A reconnect says nothing of reach and
 * pauses nothing.
 *
 * A changed person or admin mode is neither: it goes through the session reset, which empties first.
 */
export async function repairPeople(cache: QueryClient, supplied: readonly QueryKey[] = [], cut?: number, reachChanged = false): Promise<void> {
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
    && !supplied.some((key) => JSON.stringify(key) === JSON.stringify(query.queryKey))
    // A displayed native aggregate owns its supplied projections. Rebuild that
    // owner once; its child observers cannot race it with independent repairs.
    && (() => {
      const owner = scheduledReads.owner(query.queryKey);
      return owner === undefined || !isPeopleProjection(owner) || JSON.stringify(owner) === JSON.stringify(query.queryKey)
        || cache.getQueryCache().find({ queryKey: owner, exact: true })?.isActive() !== true;
    })());
  await Promise.all(queries.map((query) => cache.cancelQueries({ queryKey: query.queryKey, exact: true })));
  // Cancellation yields too: a mode write started in that interval owns the
  // next reader just as one already pending when the hint arrived does.
  await modeSettled(cache);
  if (!current()) return;
  const reads = [];
  const paused: QueryKey[] = [];
  for (const query of queries) {
    if (cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query) continue;
    if (!query.isActive() || !queryAuthorityAllows(cache, query.meta)) {
      cache.removeQueries({ queryKey: query.queryKey, exact: true });
    } else if (reachChanged && typeof query.queryKey[0] === 'string' && REVEALING.has(query.queryKey[0])) {
      query.setState({ data: undefined, error: null, status: 'pending', fetchStatus: 'idle' });
      paused.push(query.queryKey);
    } else {
      reads.push(query.queryKey);
    }
  }
  // Start an already-active owned aggregate before its dependent Node, even when Node was cached first.
  const aggregates = new Map(reads.flatMap((key) => key[0] === 'node-chat-opening' && typeof key[1] === 'string'
    && hasDefaultsOpeningOwner(cache, key) ? [[key[1], key] as const] : []));
  const pending = new Map<QueryKey, Promise<void>>();
  // A view whose re-read is refused loses what it held at that moment.
  const refused = (key: QueryKey) => {
    const query = cache.getQueryCache().find({ queryKey: key, exact: true });
    if (query?.state.status === 'error' && refuses(query.state.error)) query.setState({ data: undefined, dataUpdatedAt: 0 });
  };
  const start = (key: QueryKey) => { if (!pending.has(key)) pending.set(key, current() ? cache.refetchQueries({ queryKey: key, exact: true }).then(() => refused(key)) : Promise.resolve()); };
  // The screens that hide what a person may do with an org until its record answers stop observing the views under them while they
  // are hidden, and a view that is no longer observed stops a read of it that is under way and goes back to what it held: so those
  // records are read first, and the views are read when the screens are shown again.
  const updates = (key: QueryKey) => cache.getQueryCache().find({ queryKey: key, exact: true })?.state.dataUpdateCount;
  const before = new Map([...paused, ...reads].map((key) => [key, updates(key)] as const));
  for (const key of paused) start(key);
  await Promise.all(pending.values());
  for (const key of reads) {
    const aggregate = key[0] === 'node' && typeof key[1] === 'string' ? aggregates.get(key[1]) : undefined;
    if (aggregate !== undefined) start(aggregate);
    start(key);
  }
  await Promise.all(pending.values());
  // A read that was stopped by the view going out of sight for a moment (nothing observed it, so it went back to what it held) has not
  // answered: it is asked once more, now that the views are in sight, and not left showing what it showed before the act.
  const unanswered = [...paused, ...reads].filter((key) => {
    const query = cache.getQueryCache().find({ queryKey: key, exact: true });
    return query?.state.fetchStatus === 'idle' && query.state.status !== 'error' && query.state.dataUpdateCount === before.get(key);
  });
  if (unanswered.length > 0 && current()) await Promise.all(unanswered.map((key) => cache.refetchQueries({ queryKey: key, exact: true }).then(() => refused(key))));
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
