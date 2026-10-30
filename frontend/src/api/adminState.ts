/** Native admin answers update loaded rows; missing filtered pages remain server projections. */
import { CancelledError, QueryObserver, type Query, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { record } from '@/api/cache';

export interface AdminOrgRestoreChange { adminOrgRestore: true }
export interface AdminUserChange { adminUser: { id: string; value?: Record<string, unknown> } }
export interface MetaChange { meta: true; requestId?: string }

/** Only these stored values contribute to Meta (routes/meta.rs and sign_in::system_policy). */
export const META_SETTINGS = new Set(['system.banner', 'storage.max_blob_bytes', 'registration.mode', 'orgs.created_by', 'auth.local_login', 'auth.email_code']);

function rows(data: unknown): Record<string, unknown>[] {
  if (!record(data)) return [];
  if (Array.isArray(data.pages)) return data.pages.flatMap(rows);
  return Array.isArray(data.items) ? data.items.filter(record) : [];
}
function replace(data: unknown, id: string, value: Record<string, unknown> | null): unknown {
  if (!record(data)) return data;
  if (Array.isArray(data.pages)) return { ...data, pages: data.pages.map((page: unknown) => replace(page, id, value)) };
  if (!Array.isArray(data.items)) return data;
  return { ...data, items: data.items.flatMap((row: unknown) => record(row) && row.id === id ? value === null ? [] : [value] : [row]) };
}
function excluded(key: QueryKey, value: Record<string, unknown>): boolean {
  if (key[1] !== 'users' || !record(key[2])) return false;
  const filter = key[2];
  return typeof filter.status === 'string' && filter.status !== '' && filter.status !== value.status
    || filter.admins === true && value.is_system_admin !== true;
}

/** AdminUserUpdate cannot rename a person. Known q membership survives; status/admin cuts may not.
 * Unknown membership, ordering and cursor boundaries are obtained from the existing exact query.
 */
export async function applyAdminUser(cache: QueryClient, change: AdminUserChange['adminUser'], current: () => boolean): Promise<void> {
  const queries = cache.getQueryCache().getAll().filter(({ queryKey }) => queryKey[0] === 'admin' && (queryKey[1] === 'users' || queryKey[1] === 'user-search'));
  const known = queries.filter((query) => rows(query.state.data).some((row) => row.id === change.id));
  const relevant = change.value === undefined ? known : queries;
  // A pre-act HTTP page must not overwrite the accepted row after this act resolves.
  for (const query of relevant) {
    if (query.state.fetchStatus !== 'idle') await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (!current()) return;
  }
  if (change.value !== undefined) {
    for (const query of queries) {
      if (!current()) return;
      const present = rows(query.state.data).some((row) => row.id === change.id);
      const leaves = excluded(query.queryKey, change.value);
      if (present) cache.setQueryData(query.queryKey, replace(query.state.data, change.id, leaves ? null : change.value));
      // A native DTO does not state where a newly eligible/new person falls in a loaded page.
      if (!present || leaves) {
        if (query.isActive()) await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
        else cache.removeQueries({ queryKey: query.queryKey, exact: true });
      }
    }
    return;
  }
  // Invitation acts return 204 and there is no GET /admin/users/{id}. Read one currently
  // observed page containing the person; its invitation timestamp is the native answer.
  const source = known.find((query) => query.isActive());
  if (source === undefined) {
    for (const query of known) cache.removeQueries({ queryKey: query.queryKey, exact: true });
    return;
  }
  await cache.refetchQueries({ queryKey: source.queryKey, exact: true });
  if (!current() || source.state.status === 'error') return;
  const value = rows(source.state.data).find((row) => row.id === change.id);
  for (const query of known) {
    if (!current()) return;
    if (query === source) continue;
    if (value !== undefined) cache.setQueryData(query.queryKey, replace(query.state.data, change.id, value));
    else if (query.isActive()) await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
    else cache.removeQueries({ queryKey: query.queryKey, exact: true });
  }
}

/** A restored team root can return itself, but not the paged live/deleted membership of its subtree.
 * Keep each observed native filter/cursor projection; unopened pages expire without a read.
 */
export async function repairAdminOrgs(cache: QueryClient, current: () => boolean): Promise<void> {
  const queries = cache.getQueryCache().getAll().filter(({ queryKey }) => queryKey[0] === 'admin' && queryKey[1] === 'orgs' && (!record(queryKey[2]) || queryKey[2].orgKind !== 'personal'));
  for (const query of queries) {
    if (!current()) return;
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); continue; }
    await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (!current()) return;
    if (cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query) continue;
    if (query.isActive()) await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
    else cache.removeQueries({ queryKey: query.queryKey, exact: true });
  }
}

/** A fixed-memory echo window, not an authority, pagination or freshness limit. Unknown/evicted
 * causes always refresh. Active tickets are separate and are never evicted by this bound.
 */
const META_COMPLETED_IDS = 256;
interface MetaTicket { requestId?: string; done: Promise<void>; finish: () => void }
interface MetaRepairs {
  current: () => boolean;
  active: Map<string, MetaTicket>;
  completed: Set<string>;
  pending: MetaTicket[];
  running: boolean;
}
const metaRepairs = new WeakMap<Query, MetaRepairs>();

function completeMeta(state: MetaRepairs, ticket: MetaTicket, remember = true): void {
  if (ticket.requestId !== undefined) {
    state.active.delete(ticket.requestId);
    if (remember) {
      state.completed.delete(ticket.requestId); state.completed.add(ticket.requestId);
      while (state.completed.size > META_COMPLETED_IDS) {
        const oldest = state.completed.values().next().value;
        if (oldest !== undefined) state.completed.delete(oldest);
      }
    }
  }
  ticket.finish();
}

function startMeta(cache: QueryClient, query: Query, state: MetaRepairs): void {
  state.running = true;
  const work = async () => {
    if (!state.current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query || !query.isActive()) {
      for (const ticket of state.pending.splice(0)) completeMeta(state, ticket, false);
      return;
    }
    // Install the imperative owner synchronously, before an AuthFrame observer can leave.
    const holder = new QueryObserver(cache, { queryKey: query.queryKey,
      ...(query.options.queryFn === undefined ? {} : { queryFn: query.options.queryFn }), enabled: false, staleTime: Infinity });
    const release = holder.subscribe(() => undefined);
    const current = () => state.current() && cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) === query && query.isActive();
    try {
      // This existing flight predates the received causes. It is not their post-commit repair.
      if (query.state.fetchStatus !== 'idle') await query.promise;
      while (state.pending.length > 0 && current()) {
        const batch = state.pending.splice(0);
        let canceled = false;
        try {
          const refreshed = cache.refetchQueries({ queryKey: query.queryKey, exact: true }, { cancelRefetch: false });
          // refetchQueries may swallow a cancellation/revert; the raw flight preserves it.
          await Promise.all([refreshed, query.state.fetchStatus === 'idle' ? undefined : query.promise]);
        } catch (error) {
          canceled = error instanceof CancelledError;
          // The original query owns its error. Only a distinct pending cause may ask next.
        } finally { for (const ticket of batch) completeMeta(state, ticket); }
        if (canceled) break;
        // Causes received after this flight began require another native read. Identical IDs
        // never enter this queue; they share the active/completed ticket above.
      }
    } catch {
      // Explicit cancellation of the preceding reader is not an automatic retry.
    } finally {
      for (const ticket of state.pending.splice(0)) completeMeta(state, ticket, false);
      // A hint left without a visible reader must not leave a fresh-looking pre-change cache.
      if (state.current() && cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) === query && !query.isActive()) {
        cache.removeQueries({ queryKey: query.queryKey, exact: true });
      }
      release();
    }
  };
  const finished = () => {
    state.running = false;
    if (state.pending.length > 0) startMeta(cache, query, state);
  };
  void work().then(finished, () => {
    for (const ticket of state.pending.splice(0)) completeMeta(state, ticket, false);
    state.running = false;
  });
}

/** A native request ID joins an actor receipt and its echoed hint. Distinct causes known before
 * a read starts share that read; a new cause during it queues a fresh read. No-ID hints are new.
 */
export async function repairMeta(cache: QueryClient, current: () => boolean, requestIds: readonly (string | undefined)[] = [undefined]): Promise<void> {
  const query = cache.getQueryCache().find({ queryKey: ['meta'], exact: true });
  if (query === undefined) return;
  if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
  let state = metaRepairs.get(query);
  if (!state?.current()) {
    state = { current, active: new Map(), completed: new Set(), pending: [], running: false };
    metaRepairs.set(query, state);
  }
  const repairs = state;
  const waiting = requestIds.map((raw) => {
    const requestId = raw === '' ? undefined : raw;
    if (requestId !== undefined) {
      if (repairs.completed.delete(requestId)) { repairs.completed.add(requestId); return Promise.resolve(); }
      const active = repairs.active.get(requestId); if (active !== undefined) return active.done;
    }
    let finish!: () => void;
    const done = new Promise<void>((resolve) => { finish = resolve; });
    const ticket: MetaTicket = { ...(requestId === undefined ? {} : { requestId }), done, finish };
    repairs.pending.push(ticket); if (requestId !== undefined) repairs.active.set(requestId, ticket);
    return done;
  });
  if (repairs.pending.length > 0 && !repairs.running) startMeta(cache, query, repairs);
  await Promise.all(waiting);
}
