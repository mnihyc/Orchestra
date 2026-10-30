/** The live connection of the application: its one socket, tied to the session, and the feeds that views follow through it.
 *
 * The socket lives as long as somebody is signed in: it starts when they sign in, says who it is
 * again whenever their token is renewed, and ends, with everything it delivered, when they sign
 * out. A view says which feed it shows by a hook, and the feed is followed for as long as any view
 * does.
 */
import { isThreadDisplay } from '@/lib/threadDisplay';
import { CancelledError } from '@tanstack/react-query';
import { useEffect } from 'react';
import { useStore } from 'zustand';

import { applyChanges, type Change, followWith, people, resourceChanges, treeChanges, userEventChanges } from '@/api/acts';
import { feedSequences, record } from '@/api/cache';
import { client } from '@/api/client';
import { capturePeopleHint } from '@/api/peopleChanges';
import { fetchTree, splitListed, type TreeRows } from '@/api/nodes';
import { fetchDocumentSync } from '@/api/documents';
import { fetchNotifications, fetchSummary } from '@/api/notifications';
import { queryClient } from '@/api/query';
import { wantedResourceFeeds } from '@/realtime/resourceWants';
import { sessionsQuery } from '@/api/users';
import { fetchItem } from '@/api/threads';
import { captureThreadRead } from '@/api/threadRead';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { isDefaultThreadRead, threadReadRegistryKey, threadReadTarget, type ThreadReadTarget } from '@/realtime/threadScope';
import { applyThreadFeedRows, coordinateThreadReads, createThreadReadFeed, forgetThreadRead as forgetRead, type ThreadFeedData } from '@/realtime/threadFeed';
import type { DocumentBlock, DocumentDiscussion, DocumentRef, Notification, UserRef } from '@/generated/api-types';
import { createRealtimeSocket, type SocketStatus, type Subscription } from '@/realtime/socket';
import { type DocumentRows, documentWriter } from '@/realtime/stores/document';
import { setGapHandler, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { type Counters, onUserEvent, tellUserEvent, useUserStore, userWriter } from '@/realtime/stores/user';
import { outsideQuery } from '@/api/away';
import { type DurableFeed, FeedRegistry, followFeed, loadWithRetries } from '@/realtime/sync';
import { installQueryFeeds } from '@/realtime/queryFeeds';
import { clearResourceFollowing, setResourceFollowing } from '@/realtime/resourceFollowing';

export const socket = createRealtimeSocket({
  token: client.accessToken,
  renew: client.renew,
  // The session is over on the server: asking for a token finds that out, and the page follows.
  onSuspended: () => void client.renew().catch(() => undefined),
});

export const feeds = new FeedRegistry();
let stopQueryFeeds = installQueryFeeds(socket, queryClient);

client.onSession((event) => {
  if (event.type === 'signed_in') {
    // adopt() can replace a person without an intervening signed_out event. A subscription
    // captured for the new reader must never travel on the former person's authenticated socket.
    socket.stop();
    stopQueryFeeds();
    feedSequences.clear();
    threadWriter.rebaseAll(client.session.getState().readerVersion);
    feeds.rebase();
    stopQueryFeeds = installQueryFeeds(socket, queryClient);
    socket.start();
  }
  else if (event.type === 'renewed') socket.reauth();
  else {
    socket.stop();
    stopQueryFeeds();
    feedSequences.clear();
    feeds.clear();
    treeWriter.clear();
    threadWriter.clear();
    documentWriter.clear();
    userWriter.clear();
  }
});

// Proof updates the same server-side session the socket uses. Its recorded access refusals
// identify the feeds to ask for again; an ordinary token renewal grants no such retry.
client.onAct((act) => {
  if (act.method === 'POST' && (act.path === '/me/second-step/verify' || act.path === '/me/totp/enable')) socket.retryRefused();
});

// A session that was found before this module arrived is a session all the same.
if (client.session.getState().status === 'signed_in') socket.start();

if (typeof window !== 'undefined') {
  // The browser says the line is back: connect now, and do not wait out the pause.
  window.addEventListener('online', () => socket.wake());
}

/** How the connection stands: for the status bar and the notice above the work. */
export function useSocketStatus(): SocketStatus {
  return useStore(socket.status);
}

// ---------------------------------------------------------------- what others changed, told to the views

/**
 * How long changes told by the live connection are collected before the views ask again: a burst of frames (a
 * thread that runs restamps its row many times a second) is one question. The same pause as a person's typing
 * in a search is waited for, which nobody notices as a delay.
 */
const SETTLE_MS = 400;

let settling: Change[] = [];
let settlingReader: number | undefined;
let settleTimer: ReturnType<typeof setTimeout> | undefined;

/** Applies what the live connection says changed, through the registry of acts, once a burst has settled. */
function settle(changes: readonly Change[]): void {
  if (changes.length === 0) return;
  const reader = client.session.getState().readerVersion;
  if (reader !== settlingReader) { settling = []; settlingReader = reader; }
  settling.push(...changes.map((change) => 'people' in change ? { ...change, cut: capturePeopleHint(queryClient) } : change));
  if (settleTimer !== undefined) return;
  settleTimer = setTimeout(() => {
    const gathered = settling;
    settling = [];
    settleTimer = undefined;
    if (client.session.getState().readerVersion === settlingReader) void applyChanges(gathered);
  }, SETTLE_MS);
}

const treeKey = (orgId: string) => `org:${orgId}`;

function treeFeed(orgId: string): DurableFeed<TreeRows> {
  return {
    feed: treeKey(orgId),
    resetting: () => treeWriter.evict(orgId),
    load: async (since, signal) => {
      const reply = await loadWithRetries(() => fetchTree(orgId, since, signal), signal);
      return { seq: reply.seq, complete: reply.complete, data: { nodes: reply.nodes, hidden: reply.hidden } };
    },
    reset: (rows, seq) => treeWriter.reset(orgId, rows.nodes, seq, rows.hidden),
    apply: (rows, seq) => {
      // What the frame changes is read against the rows held before it; a tree's first load changes nothing a view has read.
      const held = useTreeStore.getState().byOrg.get(orgId);
      if (held?.loaded === true) settle(treeChanges(held.nodes, rows.nodes));
      treeWriter.apply(orgId, rows.nodes, seq, rows.hidden);
    },
    // A private place the reader administers and may not view arrives as its minimal row, kept apart.
    read: (data) => splitListed(Array.isArray(data.nodes) ? (data.nodes as unknown[]) : []),
    failed: (error) => treeWriter.failed(orgId, error),
    // The org is gone, or closed to this reader: nothing of it stays.
    closed: () => treeWriter.evict(orgId),
  };
}

/** Follows the tree of a root org for as long as the calling view is shown. */
export function useOrgTreeFeed(orgId: string | null): void {
  useEffect(() => {
    if (orgId === null) return;
    return feeds.use(
      treeKey(orgId),
      () => followFeed(socket, treeFeed(orgId)),
      () => treeWriter.evict(orgId),
    );
  }, [orgId]);
}

/** Loads a tree again from nothing, when its reader asks after a failure. */
export function reloadOrgTree(orgId: string): void {
  feeds.reload(treeKey(orgId));
}

/**
 * Asks a followed tree for what changed since it was last read. A screen does this after an act of
 * its own, so that the left bar shows it whether or not the live connection says the same.
 */
export function refreshOrgTree(orgId: string): void {
  feeds.refresh(treeKey(orgId));
}

// ---------------------------------------------------------------- a thread

const threadKey = (threadId: ThreadReadTarget) => threadReadRegistryKey(threadId);

/** The threads that a view shows at the moment, so that the server is told what is on screen. */
const shown = new Map<string, number>();

function tellVisible(threadId: string): void {
  const on = (shown.get(threadId) ?? 0) > 0 && typeof document !== 'undefined' && document.visibilityState === 'visible';
  socket.visible(`thread:${threadId}`, on);
}

function takeThreadRows(threadId: ThreadReadTarget, data: ThreadFeedData, reset: boolean): void {
  const before = applyThreadFeedRows(threadId, data, reset);
  if (before === null) return;
  const previous = before.thread?.seq;
  // The row's sequence is the existing settings signal, including a thread without history.
  // Coalesce these independent reads; item/delta frames alone do not ask them again.
  const current = threadViewOf(threadId);
  const active = isDefaultThreadRead(threadId) && (current.read?.selection === null || current.read === null);
  if (active && data.thread !== undefined && data.thread !== null && (previous === undefined || data.thread.seq > previous)) settle([{ thread: data.thread }]);
  const running = active ? data.turns?.find((turn) => turn.state === 'running' && turn.id === current.thread?.current_turn_id) : undefined;
  if (running !== undefined && before.turns.get(running.id)?.state !== 'running') {
    if (!reset) {
      // The current thread frame does not carry these stored model-input/instruction projections.
      // Repair only the exact visible thread/turn, once when a new turn actually starts.
      for (const query of queryClient.getQueryCache().getAll()) {
        const key = query.queryKey;
        if (!query.isActive() || key[1] !== threadReadTarget(threadId).threadId) continue;
        if (key[0] === 'thread-instructions' || key[0] === 'adopt-history' || key[0] === 'model-inputs' && (key[2] === null || key[2] === running.id)) void queryClient.refetchQueries({ queryKey: key, exact: true });
      }
    }
  }
}

/** Thread read scopes share the ordinary thread feed registry lifetime. */
function threadFeed(threadId: ThreadReadTarget): DurableFeed<ThreadFeedData> {
  return createThreadReadFeed(threadId, {
    rows: (data, reset) => takeThreadRows(threadId, data, reset),
    subscribed: () => tellVisible(threadReadTarget(threadId).threadId),
  });
}

// Streaming and visible HTTP-only demand share the same cache/opening coordinator.
const reloadThreadRead = coordinateThreadReads(feeds,(_threadId,ref) => ({
  rows: (data,reset) => takeThreadRows(ref,data,reset),
}));

// A gap's HTTP body belongs to the same selected observation that requested it.
setGapHandler((threadId, itemId, original) => {
  const captured = original ?? captureThreadRead(threadId);
  fetchItem(captured.ref, itemId, undefined, captured).then(
    (item) => threadWriter.itemFetched(captured.ref, item, captured),
    () => threadWriter.gapFailed(captured.ref, itemId, captured),
  );
});

if (typeof document !== 'undefined') {
  // The tab is shown or hidden: the server counts who is looking by it.
  document.addEventListener('visibilitychange', () => {
    for (const threadId of shown.keys()) tellVisible(threadId);
  });
}

/** Follows a thread for as long as the calling view is shown, and tells the server that it is on screen. */
export function useThreadFeed(threadId: ThreadReadTarget | null, reportVisible = true): void {
  const ref = useThreadReadRef(threadId ?? '');
  useEffect(() => {
    if (threadId === null) return;
    const actual = ref.threadId;
    const release = feeds.use(
      threadKey(ref),
      () => followFeed(socket, threadFeed(ref)),
      () => threadWriter.evict(ref),
      () => reloadThreadRead.rebase(ref),
    );
    if (reportVisible) {
      shown.set(actual, (shown.get(actual) ?? 0) + 1);
      tellVisible(actual);
    }
    return () => {
      if (reportVisible) {
        const left = (shown.get(actual) ?? 1) - 1;
        if (left <= 0) shown.delete(actual);
        else shown.set(actual, left);
        socket.visible(`thread:${actual}`, left > 0);
      }
      release();
    };
  }, [threadId, ref, reportVisible]);
}

/** Loads a thread again from nothing, when its reader asks after a failure. */
export function reloadThread(threadId: ThreadReadTarget): void {
  reloadThreadRead(threadId);
}

/** Destroy one local window on true tab close; other views of its native thread stay owned. */
export function forgetThreadRead(target: ThreadReadTarget): void {
  forgetRead(feeds,target);
}

/** Asks a followed thread for what changed since it was last read: after an act of the person's own. */
export function refreshThread(threadId: ThreadReadTarget): void {
  feeds.refresh(threadKey(threadId));
}

// ---------------------------------------------------------------- documents

const documentKey = (id: string) => `doc:${id}`;

function documentFeed(id: string): DurableFeed<DocumentRows> {
  return {
    feed: documentKey(id),
    invalidated: () => documentWriter.evict(id),
    load: async (since, signal) => {
      const reply = await loadWithRetries(() => fetchDocumentSync(id, since, signal), signal);
      return { seq: reply.seq, complete: reply.complete, data: { document: reply.document, blocks: reply.blocks, discussions: reply.discussions } };
    },
    reset: (rows, seq) => documentWriter.reset(id, rows, seq),
    apply: (rows, seq) => documentWriter.apply(id, rows, seq),
    read: (data) => ({
      document: (data.document ?? null) as DocumentRef | null,
      blocks: Array.isArray(data.blocks) ? (data.blocks as DocumentBlock[]) : [],
      discussions: Array.isArray(data.discussions) ? (data.discussions as DocumentDiscussion[]) : [],
    }),
    failed: (error) => documentWriter.failed(id, error),
    closed: (reason) => documentWriter.ended(id, reason),
    other: (frame) => {
      if (frame.op === 'presence') documentWriter.presence(id, frame.users);
      else if (frame.op === 'event' && frame.data.what === 'typing' && typeof frame.data.at === 'string') {
        documentWriter.typing(id, frame.data.user as UserRef, frame.data.at, Date.now());
      }
    },
  };
}

/** Follows a document's rows, who has it open and who is writing, for as long as the calling view is shown. */
export function useDocumentFeed(id: string | null): void {
  useEffect(() => {
    if (id === null) return;
    return feeds.use(
      documentKey(id),
      () => followFeed(socket, documentFeed(id)),
      () => documentWriter.evict(id),
    );
  }, [id]);
}

/** Loads a document again from nothing, when its reader asks after a failure. */
export function reloadDocument(id: string): void {
  feeds.reload(documentKey(id));
}

/** Asks a followed document for what changed since it was last read: after an act of the person's own that the feed may not say. */
export function refreshDocument(id: string): void {
  feeds.refresh(documentKey(id));
}

/** Tells the others that the person is writing in a block of a document. The server passes it on at most every few seconds. */
export function sendWriting(id: string, block: string): void {
  socket.typing(documentKey(id), block);
}

// ---------------------------------------------------------------- the person's own feed

const USER_FEED = 'user';
const INBOX_FIRST_PAGE = ['inbox', 'first-page'] as const;

/** The confirmed user cut owns this page independently of whether an Inbox pane is open. */
function retireInboxPage(): void {
  const query = queryClient.getQueryCache().find({ queryKey: INBOX_FIRST_PAGE, exact: true });
  if (query !== undefined) void query.cancel({ revert: false, silent: true });
  queryClient.removeQueries({ queryKey: INBOX_FIRST_PAGE, exact: true });
}

/** Both first-page consumers share the query and check its identity before publishing. */
function readInboxPage(staleTime: number) {
  const promise = queryClient.query({ queryKey: INBOX_FIRST_PAGE, queryFn: ({ signal }) => fetchNotifications({}, signal), staleTime });
  const owner = queryClient.getQueryCache().find({ queryKey: INBOX_FIRST_PAGE, exact: true });
  return { promise, current: () => owner !== undefined && queryClient.getQueryCache().find({ queryKey: INBOX_FIRST_PAGE, exact: true }) === owner };
}


/** What a load of the Inbox, or a frame of its feed, brings: rows, and the counters of that moment when it has them. */
interface UserRows {
  notifications: Notification[];
  counters: Counters | null;
  nextCursor: string | null;
  /** A summary seeds counters without pretending the Inbox's first page has been loaded. */
  page?: boolean;
}

let inboxReaders=0;

const NOTHING: UserRows = { notifications: [], counters: null, nextCursor: null };

/** The counters of a frame, or `null` where it carries none or not in the shape the contract says. */
function countersOf(summary: unknown): Counters | null {
  if (typeof summary !== 'object' || summary === null) return null;
  const { unread, approvals_pending: waiting } = summary as { unread?: unknown; approvals_pending?: unknown };
  return typeof unread === 'number' && typeof waiting === 'number' ? { unread, approvalsPending: waiting } : null;
}

function userFeed(): DurableFeed<UserRows> {
  let subscribed = false;
  return {
    feed: USER_FEED,
    invalidated: () => { retireInboxPage(); userWriter.clear(); settle([...people(true),{approvalViews:true}]); },
    initial: () => {
      const summary = queryClient.getQueryData(outsideQuery().queryKey);
      if (summary === undefined) return null;
      const held = useUserStore.getState();
      const newer=held.lastSeq>summary.seq;
      return { seq: newer?held.lastSeq:summary.seq, complete: true, data: { notifications: [...held.rows.values()], counters: newer?held.counters:{ unread: summary.unread, approvalsPending: summary.approvals_pending }, nextCursor: held.nextCursor } };
    },
    load: async (since, signal) => {
      // What changed after a number cannot be asked for: a repair loads the Inbox again whole, which the follower does when told so.
      if (since !== null) return { seq: since, complete: false, data: NOTHING };
      // The counters go first: the number they come with is where the subscription begins, so a row written between
      // the two requests is told again by it and never lost.
      const summary = await loadWithRetries(() => fetchSummary(signal), signal);
      signal.throwIfAborted();
      queryClient.setQueryData(outsideQuery().queryKey,summary);
      if(inboxReaders===0) return {seq:summary.seq,complete:true,data:{notifications:[],counters:{unread:summary.unread,approvalsPending:summary.approvals_pending},nextCursor:null,page:false}};
      const read = readInboxPage(0);
      const page = await read.promise;
      signal.throwIfAborted();
      if (!read.current()) throw new CancelledError({ silent: true });
      return {
        seq: summary.seq,
        complete: true,
        data: {
          notifications: page.items,
          counters: { unread: summary.unread, approvalsPending: summary.approvals_pending },
          nextCursor: page.next_cursor,
          page:true,
        },
      };
    },
    reset: (data, seq) => data.page===false ? userWriter.apply(data.notifications,data.counters,seq) : userWriter.reset(data.notifications, data.counters ?? { unread: 0, approvalsPending: 0 }, seq, data.nextCursor),
    apply: (data, seq) => {
      userWriter.apply(data.notifications,data.counters,seq);
      const summary=queryClient.getQueryData(outsideQuery().queryKey);
      if(summary!==undefined && data.counters!==null) queryClient.setQueryData(outsideQuery().queryKey,{...summary,seq,unread:data.counters.unread,approvals_pending:data.counters.approvalsPending});
    },
    read: (data) => ({
      notifications: Array.isArray(data.notifications) ? (data.notifications as Notification[]) : [],
      counters: countersOf(data.summary),
      nextCursor: null,
    }),
    failed: (error) => userWriter.failed(error),
    // News is never replayed: after every connection, whoever depends on some loads it again.
    subscribed: (frame) => {
      // Reload is a refusal of this cursor, not an admitted source. An accepted
      // source closes the authority gap before initial admission or reconnect:
      // personal events are not replayed and an Inbox sequence is not that cut.
      if (frame.mode === 'reload') return;
      settle(people(true));
      tellUserEvent('connected', { resumed: subscribed }); subscribed = true;
    },
    other: (frame) => {
      if (frame.op === 'event') {
        const what = typeof frame.data.what === 'string' ? frame.data.what : '';
        const counters = countersOf(frame.data.summary);
        if (counters !== null) userWriter.counters(counters);
        // A consumer of personal preferences reads this exact value, already cached, before its callback.
        if (what === 'state') void applyChanges(userEventChanges(what, frame.data));
        tellUserEvent(what, frame.data);
      }
    },
  };
}

/** Reads the two counters again, for news that moves one of them without a row of the Inbox. */
export function refreshCounters(): void {
  fetchSummary().then(
    (summary) => userWriter.counters({ unread: summary.unread, approvalsPending: summary.approvals_pending }),
    // The next frame or the next connection brings them right.
    () => undefined,
  );
}

/**
 * The person's news that changes what views read, told through the registry of acts: an approval made or left waiting,
 * a route changed without them, their preferences or Inbox changed on another device, a machine or a resource of
 * theirs that changed state, and every new connection (news is never replayed). Their positions, levels, favorites,
 * what the interface keeps and their chats are followed by the left bar and the chats, which read them in place.
 */
const USER_NEWS = ['approvals', 'route', 'notification_prefs', 'notifications_read', 'subscription', 'machines', 'resources', 'enrollment', 'org_creation', 'meta_changed', 'connected'] as const;
for (const what of USER_NEWS) onUserEvent(what, (data) => settle(userEventChanges(what, data)));
onUserEvent('continuation_read_state', (data) => {
  if (typeof data.thread_id === 'string' && typeof data.continuation_id === 'string' && typeof data.last_read_seq === 'number') {
    threadWriter.readStateAll(data.thread_id, data.continuation_id, data.last_read_seq);
    if (isThreadDisplay(data.thread_display)) { threadWriter.display(data.thread_id, data.thread_display); treeWriter.display(data.thread_id, data.thread_display); }
  }
});
onUserEvent('read_state', (data) => {
  if (typeof data.thread_id === 'string' && typeof data.last_read_seq === 'number') {
    threadWriter.readStateAll(data.thread_id, null, data.last_read_seq);
    if (isThreadDisplay(data.thread_display)) { threadWriter.display(data.thread_id, data.thread_display); treeWriter.display(data.thread_id, data.thread_display); }
  }
});
onUserEvent('admin_mode', (data) => {
  // Other sessions of the same person retain their own switches and live projections.
  if(queryClient.isMutating({mutationKey:['admin-mode']})>0 || queryClient.getQueryData<{as_admin:boolean}>(['admin-mode'])?.as_admin===data.as_admin) return;
  void queryClient.query(sessionsQuery).then((sessions) => {
    if (sessions.some((session) => session.current && session.id === data.session_id)) void applyChanges(userEventChanges('admin_mode', data));
  }).catch(() => undefined);
});

// What an act changed that a live feed keeps is asked of that feed: the trees held, a thread, a document, the counters.
followWith({
  session: () => { socket.reauth(); socket.retryRefused(); feeds.rebase(); },
  trees: () => {
    for (const orgId of useTreeStore.getState().byOrg.keys()) feeds.refresh(treeKey(orgId));
  },
  thread: (id) => refreshThread(id),
  document: (id) => refreshDocument(id),
  counters: () => refreshCounters(),
});

// ---------------------------------------------------------------- the feeds of the resources on screen

/** The feeds of the resources whose pages are on screen, by `kind:id`. */
const resourceFeeds = new Map<string, Subscription>();
let reconciling = false;
socket.status.subscribe((status) => { if (status.phase !== 'ready') clearResourceFollowing(); });

/**
 * Follows the feed of every resource a view on screen reads (`resource:<kind>:<id>`), and only those: what its owners
 * or managers change elsewhere arrives as that recipient's native row and updates held views directly.
 * Bare general news defers to this object feed; only a genuinely missing projection needs an exact repair.
 * A feed the person may not follow is refused by the server and says nothing.
 */
function reconcileResourceFeeds(): void {
  reconciling = false;
  const wanted = wantedResourceFeeds(queryClient);
  for (const [name, subscription] of resourceFeeds) {
    if (wanted.has(name)) continue;
    subscription.unsubscribe();
    resourceFeeds.delete(name);
    const [kind = '', id = ''] = name.split(':');
    setResourceFollowing(kind, id, false);
  }
  for (const name of wanted) {
    if (resourceFeeds.has(name)) continue;
    const [kind = '', id = ''] = name.split(':');
    let seq: number | null = null;
    resourceFeeds.set(
      name,
      socket.subscribe(`resource:${name}`, {
        since: () => seq,
        onFrame: (frame) => {
          if (frame.op === 'subscribed') { setResourceFollowing(kind, id, true); return; }
          if (frame.op === 'closed' || frame.op === 'error') setResourceFollowing(kind, id, false);
          if ((frame.op === 'closed' && frame.reason !== 'unsubscribed') || (frame.op === 'error' && (frame.code === 'forbidden' || frame.code === 'not_found'))) {
            settle([{ row: { kind, id, value: null } }]);
            return;
          }
          if (frame.op === 'sync') { if (seq !== null && frame.seq <= seq) return; seq = frame.seq; }
          if (frame.op !== 'event' && frame.op !== 'sync') return;
          setResourceFollowing(kind, id, true);
          if (frame.data.what === 'login' && record(frame.data.state)) {
            settle([{ value: { key: ['provider-login', id], data: frame.data.state } }]);
          } else if (record(frame.data.job) && typeof frame.data.job.id === 'string') {
            settle([{ row: { kind: 'job', id: frame.data.job.id, value: frame.data.job } }]);
          } else if (frame.data.what === 'changed' || 'row' in frame.data || 'value' in frame.data || frame.data.deleted === true) settle(resourceChanges(kind, id, frame.data));
        },
      }),
    );
  }
}

// Which views are on screen changes as they mount and unmount: the feeds follow once the page has settled from it.
queryClient.getQueryCache().subscribe((event) => {
  if (event.type !== 'observerAdded' && event.type !== 'observerRemoved' && event.type !== 'removed' && event.type !== 'updated') return;
  if (reconciling) return;
  reconciling = true;
  queueMicrotask(reconcileResourceFeeds);
});
client.onSession((event) => {
  if (event.type !== 'signed_out' && event.type !== 'signed_in') return;
  for (const subscription of resourceFeeds.values()) subscription.unsubscribe();
  resourceFeeds.clear();
  clearResourceFollowing();
  // stop() retired these subscription objects too; current mounted resources must join the new socket.
  if (event.type === 'signed_in' && !reconciling) { reconciling = true; queueMicrotask(reconcileResourceFeeds); }
});

/** Follows the person's own feed for as long as the calling view is shown: their Inbox and its counters. */
export function useUserFeed(rows = true, enabled = true): void {
  useEffect(()=>{if(!rows || !enabled)return;inboxReaders+=1;return()=>{inboxReaders-=1;};},[rows, enabled]);
  useEffect(() => enabled ? feeds.use(USER_FEED, () => followFeed(socket, userFeed()), () => userWriter.clear()) : undefined, [enabled]);
  useEffect(() => {
    if (!enabled || !rows || useUserStore.getState().loaded) return;
    const abort = new AbortController();
    const read = readInboxPage(Infinity);
    void read.promise.then((page) => {
      if (abort.signal.aborted || !read.current()) return;
      const held = useUserStore.getState();
      userWriter.reset([...page.items, ...held.rows.values()], held.counters, held.lastSeq, page.next_cursor);
    }, (error: unknown) => { if (!abort.signal.aborted && read.current()) userWriter.failed(error); });
    return () => abort.abort();
  }, [rows, enabled]);
}

/** Loads the Inbox again from nothing, when its reader asks after a failure. */
export function reloadUserFeed(): void {
  feeds.reload(USER_FEED);
}
