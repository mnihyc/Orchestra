/** Visible HTTP queries share live subscriptions; native snapshots update their cache and cursors survive reconnects. */
import { QueryObserver, type QueryClient, type QueryKey, type QueryObserverResult } from '@tanstack/react-query';
import { applyRow, feedSequences, record } from '@/api/cache';
import { ApiError } from '@/api/client';
import { isOver, jobQuery } from '@/api/forks';
import { deliverLiveValue, refuseLiveValue } from '@/api/liveValue';
import type { EnvironmentPorts as NativePorts, Job, ResolvedSetting } from '@/generated/api-types';
import { installRawQueryFeeds } from '@/realtime/rawQuery';
import { portsView } from '@/features/ports/wire';
import type { FeedFrame, RealtimeSocket, Subscription } from '@/realtime/socket';

interface Source {
  feed: string;
  member?: string;
  /** Keep the HTTP answer's static capability fields around a smaller live snapshot. */
  merge?: boolean;
}

const isQueryKey = (value: unknown): value is QueryKey => Array.isArray(value);

/** Native feed names, shared with the server. A thread's door is retained for its access check. */
export function queryFeed(key: QueryKey): Source | null {
  const [head, id, third] = key;
  if (typeof head !== 'string' || typeof id !== 'string' || id === '') return null;
  const through = typeof third === 'string' && third !== '' ? `thread:${third}` : `env:${id}`;
  switch (head) {
    case 'environment': return { feed: `env:${id}`, member: 'environment' };
    case 'processes': return { feed: `env:${id}:procs` };
    case 'resources': return { feed: `env:${id}:stats` };
    case 'environment-browsers': return { feed: `env:${id}:browsers`, merge: true };
    case 'environment-displays': return { feed: `env:${id}:displays`, merge: true };
    case 'terminals': return { feed: `env:${id}:terminals`, member: third === 'closed' || key[3] === 'closed' ? 'closed' : 'open' };
    case 'environment-ports': return { feed: `env:${id}:ports` };
    case 'workspace-git': return { feed: typeof third === 'string' ? `${through}:git` : `env:${id}:git:`, member: 'status' };
    case 'job': return { feed: `job:${id}`, member: 'job' };
    case 'fork-batch': return { feed: `fork:${id}`, member: 'batch' };
    case 'usage': return id === 'health' && typeof third === 'string' ? { feed: `usage:provider:${third}:health`, member: 'health' } : null;
    default: return null;
  }
}

/** Apply a snapshot or append-only page of native rows, leaving older cursor pages intact. */
export function applyQueryFrame(cache: QueryClient, key: QueryKey, frame: FeedFrame): void {
  const source = queryFeed(key);
  if (source === null || (frame.op !== 'event' && frame.op !== 'sync')) return;
  const data = frame.data;
  // Ports owns a domain snapshot inside its full projection; it is not an outer feed envelope.
  const envelope = key[0] === 'environment-ports' ? data : record(data.snapshot) ? data.snapshot : data;
  if (envelope.offline === true) {
    refuseLiveValue(key, new ApiError({ type: 'orch:error:environment.unavailable', code: 'environment.unavailable', title: '', detail: '', status: 503, request_id: '' }, null));
    return;
  }
  let next: unknown = source.member === undefined ? envelope : envelope[source.member];
  if (key[0] === 'environment') {
    if (!record(next) || next.id !== key[1] || typeof next.generation !== 'number' || typeof next.workspace_root !== 'string') return;
    const query = cache.getQueryCache().find({ queryKey: key, exact: true });
    if (query !== undefined) void query.cancel({ revert: false, silent: true });
    applyRow(cache, { kind: 'environment', id: String(key[1]), value: next });
    if (frame.op === 'sync') feedSequences.set(source.feed, Math.max(feedSequences.get(source.feed) ?? 0, frame.seq));
    for (const job of Array.isArray(data.jobs) ? data.jobs as unknown[] : []) {
      if (record(job) && typeof job.id === 'string' && 'result' in job) applyRow(cache, { kind: 'job', id: job.id, value: job });
    }
    return;
  }
  if (key[0] === 'terminals' && record(next)) next = next.items;
  if (key[0] === 'environment-ports') {
    const snapshot = envelope.snapshot;
    // An observation alone, or a missing/late discovery answer, is not a complete page of ports.
    // Keep a pending read pending and a previous full view intact until its native projection arrives.
    if (envelope.environment_id !== key[1] || !record(snapshot)
      || snapshot.discovery !== 'native' && snapshot.discovery !== 'unsupported' && snapshot.discovery !== 'offline'
      || typeof snapshot.generation !== 'number' || typeof snapshot.as_of !== 'string'
      || !Array.isArray(envelope.targets) || !Array.isArray(envelope.openings)
      || !Array.isArray(envelope.allowed) || !Array.isArray(envelope.capabilities)
      || typeof envelope.limit !== 'number' || typeof envelope.used !== 'number'
      || envelope.host_ports !== null && !record(envelope.host_ports)) return;
    next = portsView(envelope as unknown as NativePorts, cache.getQueryData<ResolvedSetting[]>(['environment-settings', key[1]]) ?? []);
  }
  if (next === undefined) return;
  if (key[0] === 'job' && (!record(next) || next.id !== key[1] || typeof next.state !== 'string' || !record(next.progress))) return;
  if (source.merge === true) {
    // Browser/display feeds can carry just changing rows. They cannot seed a full HTTP projection
    // before its capability facts arrive, or replace those facts with an incomplete nested report.
    const before = cache.getQueryData(key);
    const view = record(before) && record(next) ? { ...before, ...next } : next;
    const browsers = key[0] === 'environment-browsers';
    if (!record(view) || !record(view.support) || !Array.isArray(view.support[browsers ? 'programs' : 'servers'])
      || !Array.isArray(view[browsers ? 'browsers' : 'displays']) || !('off' in view)
      || !record(view.limit) || typeof view.limit.open !== 'number' || typeof view.limit.max !== 'number'
      || browsers && (!Array.isArray(view.profiles) || !Array.isArray(view.presets) || !('profiles_dir' in view))) return;
    next = view;
  }
  const held = cache.setQueryData(key, (before: unknown) => {
    if (key[0] === 'resources' && record(next) && typeof next.at === 'string') {
      const at = next.at;
      const minutes = typeof key[2] === 'number' ? key[2] : typeof key[3] === 'number' ? key[3] : 60;
      const cutoff = Date.parse(at) - minutes * 60_000;
      const samples = Array.isArray(before) ? before as unknown[] : [];
      return [...samples.filter((one) => record(one) && typeof one.at === 'string' && one.at !== at && Date.parse(one.at) >= cutoff), next];
    }
    if (record(before) && Array.isArray(before.pages) && Array.isArray(next)) {
      const pages = before.pages as unknown[];
      const first = pages[0];
      if (!record(first)) return before;
      const field = 'items';
      const held = new Map<unknown, unknown>();
      for (const one of next as unknown[]) if (record(one)) held.set(one.id ?? one.n, one);
      for (const one of Array.isArray(first[field]) ? first[field] as unknown[] : []) if (record(one) && !held.has(one.id ?? one.n)) held.set(one.id ?? one.n, one);
      return { ...before, pages: [{ ...first, [field]: [...held.values()] }, ...pages.slice(1)] };
    }
    return source.merge === true && record(before) && record(next) ? { ...before, ...next } : next;
  });
  if (held !== undefined) deliverLiveValue(key, held);
}

/** One subscription per visible source, even when a status part and a panel read it together. */
export function installQueryFeeds(socket: RealtimeSocket, cache: QueryClient): () => void {
  const stopRaw = installRawQueryFeeds(socket, cache);
  const subscriptions = new Map<string, Subscription>();
  const cursors = new Map<string, number>();
  const repairs = new Map<string, object>();
  let queued = false;
  let stopped = false;
  const wanted = (): Map<string, QueryKey[]> => {
    const result = new Map<string, QueryKey[]>();
    for (const query of cache.getQueryCache().getAll()) {
      if (!query.isActive()) continue;
      const source = queryFeed(query.queryKey);
      if (source !== null) result.set(source.feed, [...result.get(source.feed) ?? [], query.queryKey]);
    }
    return result;
  };
  const receive = (feed: string, frame: FeedFrame): void => {
    // Capacity is a waiting admission, not a failed read; the socket promotes this same demand.
    if (frame.op === 'error' && frame.code === 'too_many_subscriptions') return;
    if (frame.op === 'sync') {
      const held = cursors.get(feed) ?? feedSequences.get(feed);
      if (held !== undefined && frame.seq <= held) return;
      cursors.set(feed, frame.seq);
    } else if (frame.op === 'resync' || frame.op === 'subscribed' && frame.mode === 'reload') {
      // Environment rows need their HTTP cut; browser/display frames omit capability facts.
      // Other sources supply a complete baseline on their next native subscription.
      const full = frame.op === 'resync' && frame.reload === true;
      const ticket = {};
      repairs.set(feed, ticket);
      cursors.delete(feed);
      feedSequences.delete(feed);
      subscriptions.get(feed)?.unsubscribe(); subscriptions.delete(feed);
      const reads: Promise<unknown>[] = [];
      for (const query of cache.getQueryCache().getAll()) {
        const source = queryFeed(query.queryKey);
        if (source?.feed !== feed) continue;
        if (full) {
          void query.cancel({ revert: false, silent: true });
          if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); continue; }
          query.setState({ data: undefined, dataUpdatedAt: 0, error: null, status: 'pending', fetchStatus: 'idle' });
        }
        if (query.isActive() && (query.queryKey[0] === 'environment' || full && source.merge === true)) {
          reads.push(!full && query.state.fetchStatus === 'fetching' && query.promise !== undefined
            ? query.promise : cache.refetchQueries({ queryKey: query.queryKey, exact: true }, { throwOnError: true }));
        }
      }
      // Fetch notifications can retire an earlier repair; this cut owns the reads just started.
      repairs.set(feed, ticket);
      const resume = (results: PromiseSettledResult<unknown>[] = []) => {
        if (stopped || repairs.get(feed) !== ticket || results.some((result) => result.status === 'rejected')) return;
        repairs.delete(feed);
        reconcile();
      };
      if (reads.length === 0) resume();
      else void Promise.allSettled(reads).then(resume);
      return;
    } else if ((frame.op === 'closed' && frame.reason !== 'unsubscribed') || frame.op === 'error') {
      const forbidden = frame.op === 'closed' && frame.reason === 'revoked' || frame.op === 'error' && frame.code === 'forbidden';
      const missing=frame.op==='closed' && frame.reason==='deleted' || frame.op==='error' && frame.code==='not_found';
      const code=forbidden?'authz.denied':missing?'not_found':'internal';
      const error = new ApiError({ type: `orch:error:${code}`, code, title: '', detail: '', status: forbidden ? 403 : missing ? 404 : 503, request_id: '' }, null);
      for (const key of wanted().get(feed) ?? []) {
        refuseLiveValue(key, error);
        const query = cache.getQueryCache().find({ queryKey: key, exact: true });
        if (query !== undefined) void query.cancel({ revert: false, silent: true });
        query?.setState({ data: undefined, error, status: 'error', fetchStatus: 'idle' });
      }
      cursors.delete(feed);
      // Keep the refused owner recorded until an explicit retry, but retire its callbacks now.
      subscriptions.get(feed)?.unsubscribe();
      return;
    }
    for (const key of wanted().get(feed) ?? []) applyQueryFrame(cache, key, frame);
  };
  const follow = (feed: string): Subscription => {
    let active = true;
    const owned = socket.subscribe(feed, { since: () => cursors.get(feed) ?? feedSequences.get(feed) ?? null,
      onFrame: (frame) => { if (active && !stopped) receive(feed, frame); },
    });
    return { unsubscribe() { if (!active) return; active = false; owned.unsubscribe(); } };
  };
  const reconcile = (): void => {
    queued = false;
    if (stopped) return;
    const needed = wanted();
    for (const [feed, subscription] of subscriptions) if (!needed.has(feed)) { subscription.unsubscribe(); subscriptions.delete(feed); }
    for (const feed of needed.keys()) if (!subscriptions.has(feed) && !repairs.has(feed)) subscriptions.set(feed, follow(feed));
  };
  const unlisten = cache.getQueryCache().subscribe((event) => {
    if (event.type === 'updated' && event.action.type === 'fetch') {
      // The cache's notification union erases the query-key generic; narrow at that boundary.
      const key: unknown = event.query.queryKey;
      const source = isQueryKey(key) ? queryFeed(key) : null;
      const subscription = source === null ? undefined : subscriptions.get(source.feed);
      if (source !== null) { repairs.delete(source.feed); subscription?.unsubscribe(); subscriptions.delete(source.feed); cursors.delete(source.feed); }
    }
    if (queued || event.type === 'observerResultsUpdated') return;
    queued = true;
    queueMicrotask(reconcile);
  });
  reconcile();
  return () => { stopped = true; unlisten(); stopRaw(); for (const subscription of subscriptions.values()) subscription.unsubscribe(); subscriptions.clear(); cursors.clear(); repairs.clear(); };
}

/** A pending act owns one observer of the same job query as a pane; the installed feed manager supplies it. */
export function waitForJob(cache: QueryClient, initial: Job, changed?: (job: Job) => void, signal?: AbortSignal): Promise<Job> {
  if (signal?.aborted === true) return Promise.reject(new DOMException('Job reader changed', 'AbortError'));
  changed?.(initial);
  if (isOver(initial.state)) {
    applyRow(cache, { kind: 'job', id: initial.id, value: { ...initial } });
    return Promise.resolve(initial);
  }
  const observer = new QueryObserver(cache, { ...jobQuery(initial.id), initialData: initial, staleTime: Infinity, retry: false });
  return new Promise((resolve, reject) => {
    let settled = false;
    const leave = () => { observer.destroy(); signal?.removeEventListener('abort', abort); };
    const abort = () => {
      if (settled) return;
      settled = true; leave(); reject(new DOMException('Job reader changed', 'AbortError'));
    };
    const receive = (result: QueryObserverResult<Job>): void => {
      if (settled) return;
      if (result.isError) {
        settled = true; leave(); reject(result.error);
      } else if (result.data?.id === initial.id) {
        changed?.(result.data);
        if (isOver(result.data.state)) {
          settled = true; leave(); resolve(result.data);
        }
      }
    };
    signal?.addEventListener('abort', abort, { once: true });
    observer.subscribe(receive);
    if (signal?.aborted === true) abort();
    receive(observer.getCurrentResult());
  });
}
