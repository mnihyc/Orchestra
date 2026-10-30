/** Authentication, reconnect, feed subscriptions, and the round trip measured for the status bar.
 *
 * One socket for the page carries every live update. This is the connection itself: it says who it
 * is with the access token, says so again when the token is renewed, keeps a list of subscriptions
 * and sends them anew after every reconnect, measures the round trip with each heartbeat, and comes
 * back by itself with growing pauses when the line goes. It never accepts a change: changes are made
 * over HTTP, and arrive here as the rows they changed.
 *
 * What a frame means for a feed is not decided here: a subscriber is handed the frames of its feed
 * (`sync.ts` holds the rules for the feeds that are kept in order).
 */
import { createStore, type StoreApi } from 'zustand/vanilla';

import type { ClientFrame, ServerFrame } from '@/generated/api-types';
import { openingId } from '@/lib/opening';

// ---------------------------------------------------------------- frames, as the protocol names them

export type { ClientFrame } from '@/generated/api-types';
export type SubscribedMode = Extract<ServerFrame, { op: 'subscribed' }>['mode'];
export type ClosedReason = Extract<ServerFrame, { op: 'closed' }>['reason'];
export type SubscriptionError = Extract<ServerFrame, { op: 'error' }>['code'];

/** What the server sends about one subscription. */
export type FeedFrame = Exclude<ServerFrame, { op: 'ready' | 'pong' }>;
type ReadyFrame = Extract<ServerFrame, { op: 'ready' }>;

/** What is parsed off the wire, before it is known to be one of the frames above. */
type RawFrame = Record<string, unknown>;

/** The kinds of frame that belong to one subscription. */
const FEED_OPS: ReadonlySet<unknown> = new Set<FeedFrame['op']>(['subscribed', 'sync', 'delta', 'presence', 'event', 'resync', 'closed', 'error']);

/** Whether a frame is one of a subscription: a kind the protocol names, and the number of the subscription it is about. A kind a newer server adds is not one to hand on. */
function isFeedFrame(frame: RawFrame): frame is RawFrame & FeedFrame & { id: number } {
  return FEED_OPS.has(frame.op) && typeof frame.id === 'number';
}

/** The frame that says the token was taken, read member by member: a member the server leaves out is what the page would have done without. */
function readyOf(frame: RawFrame): ReadyFrame {
  return {
    op: 'ready',
    connection_id: typeof frame.connection_id === 'string' ? frame.connection_id : '',
    user_id: typeof frame.user_id === 'string' ? frame.user_id : '',
    server_time: typeof frame.server_time === 'string' ? frame.server_time : '',
    access_expires_at: typeof frame.access_expires_at === 'string' ? frame.access_expires_at : '',
    heartbeat_secs: typeof frame.heartbeat_secs === 'number' ? frame.heartbeat_secs : 0,
    build: typeof frame.build === 'string' ? frame.build : '',
  };
}

// ---------------------------------------------------------------- the numbers of the protocol

/** The path of the socket under the API, and the one subprotocol it speaks. */
export const SOCKET_PATH = '/api/v1/ws';
export const SUBPROTOCOL = 'orch.v1';

/** The heartbeat, until the server says its own: a `ping` this often. */
const HEARTBEAT_MS = 25_000;
/** Either side gives a connection up after this long without a frame: slowness alone never closes one. */
const SILENCE_LIMIT_MS = 60_000;
/** The pauses before connecting again: half a second, doubling to fifteen, each a fifth more or less at random. */
const BACKOFF_FIRST_MS = 500;
const BACKOFF_MOST_MS = 15_000;
const BACKOFF_SPREAD = 0.2;
/** A connection that held this long was a good one: the next pause begins small again. */
const BACKOFF_FORGOTTEN_AFTER_MS = 30_000;
/** How many round trips the measure is the middle one of, so that one late packet changes nothing. */
const ROUND_TRIPS_KEPT = 3;

/** The close codes of the protocol that mean more than "connect again". */
const CLOSE_UNAUTHENTICATED = 4401;
const CLOSE_SUSPENDED = 4403;
const CLOSE_OTHER_BUILD = 4409;

export type SocketPhase =
  /** Not started, or stopped for good. */
  | 'idle'
  | 'connecting'
  /** Open, and waiting to be told that the token was taken. */
  | 'authenticating'
  | 'ready'
  /** Gone, and waiting out a pause before the next attempt. */
  | 'waiting';

export interface SocketStatus {
  phase: SocketPhase;
  /** When the socket last stopped being ready, on this device's clock; `null` while it is ready or was never started. */
  lostAt: number | null;
  /** The middle one of the last few round trips, in milliseconds; `null` until one was measured. */
  roundTripMs: number | null;
  /** The build the server says it is. */
  build: string | null;
  /** True once the server refused the protocol this page speaks: a newer build is deployed. */
  outdated: boolean;
}

/** What a subscriber is told. */
export interface FeedListener {
  /** Immutable selectors captured with this exact subscription, including reconnects. */
  continuationId?: string;
  projectionRevision?: number;
  /** The actual native segment; required only by a raw subscription. */
  rawSegmentId?: string;
  /** The actual new wire id, before each sent request, including reconnects. */
  onSubscribe?: (id: number) => void;
  /** Captured with this subscription and reused after reconnects. */
  openingId?: string;
  /** A frame of the feed arrived. */
  onFrame: (frame: FeedFrame) => void;
  /** The sequence to ask from when the feed is subscribed again after a reconnect; `null` for a feed that has none. */
  since: () => number | null;
  subStreams?: boolean;
}

export interface Subscription {
  unsubscribe: () => void;
}

/** The parts of a WebSocket this client uses, so that a test can stand in for one. */
export interface SocketLike {
  readyState: number;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  onerror: (() => void) | null;
  send: (data: string) => void;
  close: (code?: number) => void;
}

export interface RealtimeOptions {
  /** The address of the socket; this page's own origin when absent. */
  url?: () => string;
  /** The access token to say who this is, or `null` when nobody is signed in. */
  token: () => string | null;
  /** Asks for a new access token. Answers whether somebody is still signed in. */
  renew: () => Promise<boolean>;
  /** Called when the server says the person is suspended. */
  onSuspended: () => void;
  /** Makes the socket; the browser's own when absent. */
  connect?: (url: string, protocol: string) => SocketLike;
  now?: () => number;
  random?: () => number;
}

const OPEN = 1;

export interface RealtimeSocket {
  status: StoreApi<SocketStatus>;
  /** Connects, and from then on stays connected. */
  start: () => void;
  /** Closes the connection and forgets every subscription. */
  stop: () => void;
  /** Subscribes to a feed. The subscription is sent now if the socket is ready, and anew after every reconnect. */
  subscribe: (feed: string, listener: FeedListener) => Subscription;
  /** Tells the server whether a feed is on screen, for the presence of that feed. */
  visible: (feed: string, visible: boolean) => void;
  /** Tells the server that the person is typing in the composer of a chat's feed (or writing in a block of a document's, `at` naming the block). */
  typing: (feed: string, at?: string) => void;
  /** Says again who this is, with a token that was just renewed. */
  reauth: () => void;
  /** Retries still-wanted feeds refused by access, after the session gives its second step. */
  retryRefused: () => void;
  /** Tries to connect at once instead of waiting out the pause: the browser says the line is back. */
  wake: () => void;
}

function defaultUrl(): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${location.host}${SOCKET_PATH}`;
}

export function createRealtimeSocket(options: RealtimeOptions): RealtimeSocket {
  const now = options.now ?? (() => Date.now());
  const random = options.random ?? Math.random;
  const connect = options.connect ?? ((url, protocol) => new WebSocket(url, protocol) as unknown as SocketLike);
  const status = createStore<SocketStatus>(() => ({
    phase: 'idle',
    lostAt: null,
    roundTripMs: null,
    build: null,
    outdated: false,
  }));

  let socket: SocketLike | null = null;
  let wanted = false;
  let nextSub = 1;
  let attempts = 0;
  let readySince = 0;
  let heartbeatMs = HEARTBEAT_MS;
  let lastHeard = 0;
  let pingN = 0;
  const pings = new Map<number, number>();
  let roundTrips: number[] = [];
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let heartbeatTimer: ReturnType<typeof setInterval> | undefined;

  /** Every subscription that is wanted, by the id it has on the connection of the moment. */
  const subs = new Map<number, { feed: string; listener: FeedListener }>();
  // Refusals carry subscription ids even when a hold is indistinguishable from ordinary access
  // denial. Remember those requests until their view leaves; only successful second-step proof
  // asks again, and the server still decides their current access. A closed feed stays closed.
  const refused = new Set<{ feed: string; listener: FeedListener }>();
  // Capacity is temporary: hidden views can release a slot without reconnecting the socket.
  // Keep that wanted listener and its cursor, and retry only when a slot is actually released.
  const waitingCapacity = new Set<{ feed: string; listener: FeedListener }>();

  const send = (frame: ClientFrame): boolean => {
    if (socket?.readyState !== OPEN) return false;
    socket.send(JSON.stringify(frame));
    return true;
  };

  const sendSub = (id: number, feed: string, listener: FeedListener) => {
    if (socket?.readyState !== OPEN) return;
    listener.onSubscribe?.(id);
    const since = listener.since();
    send({
      op: 'sub',
      id,
      feed,
      opening_id: listener.openingId,
      ...(listener.continuationId === undefined ? {} : { continuation_id: listener.continuationId }),
      ...(listener.projectionRevision === undefined ? {} : { projection_revision: listener.projectionRevision }),
      ...(listener.rawSegmentId === undefined ? {} : { raw_segment_id: listener.rawSegmentId }),
      ...(since === null ? {} : { since }),
      ...(listener.subStreams === true ? { sub_streams: true } : {}),
    });
  };

  function fillReleasedSlot(): void {
    if (status.getState().phase !== 'ready') return;
    for (const [id, sub] of subs) {
      if (!waitingCapacity.delete(sub)) continue;
      sendSub(id, sub.feed, sub.listener);
      return;
    }
  }

  function lost(): void {
    clearInterval(heartbeatTimer);
    pings.clear();
    const before = status.getState();
    status.setState({ phase: wanted ? 'waiting' : 'idle', lostAt: before.lostAt ?? (wanted ? now() : null) });
  }

  function retryLater(): void {
    if (!wanted) return;
    // A connection that held was a good one: start small again.
    if (readySince !== 0 && now() - readySince >= BACKOFF_FORGOTTEN_AFTER_MS) attempts = 0;
    readySince = 0;
    const pause = Math.min(BACKOFF_MOST_MS, BACKOFF_FIRST_MS * 2 ** attempts);
    attempts += 1;
    const spread = pause * BACKOFF_SPREAD * (random() * 2 - 1);
    clearTimeout(retryTimer);
    retryTimer = setTimeout(open, pause + spread);
  }

  function heartbeat(): void {
    if (now() - lastHeard > SILENCE_LIMIT_MS) {
      // Nothing for a minute: the connection is gone, whatever the browser believes, and is not
      // waited for to say so itself.
      const dead = socket;
      socket = null;
      dead?.close();
      lost();
      retryLater();
      return;
    }
    pingN += 1;
    pings.set(pingN, now());
    send({ op: 'ping', n: pingN });
  }

  function measured(sentAt: number): void {
    roundTrips = [...roundTrips, now() - sentAt].slice(-ROUND_TRIPS_KEPT);
    const sorted = [...roundTrips].sort((one, other) => one - other);
    status.setState({ roundTripMs: sorted[Math.floor(sorted.length / 2)] ?? null });
  }

  function onReady(frame: ReadyFrame, authSentAt: number): void {
    readySince = now();
    heartbeatMs = frame.heartbeat_secs > 0 ? frame.heartbeat_secs * 1_000 : HEARTBEAT_MS;
    measured(authSentAt);
    status.setState({ phase: 'ready', lostAt: null, build: frame.build });
    clearInterval(heartbeatTimer);
    heartbeatTimer = setInterval(heartbeat, heartbeatMs);
    // Every feed that is wanted is subscribed again, from where it had come to.
    const wantedSubs = [...subs.values()];
    subs.clear();
    waitingCapacity.clear();
    for (const sub of wantedSubs) {
      const id = nextSub;
      nextSub += 1;
      subs.set(id, sub);
      sendSub(id, sub.feed, sub.listener);
    }
  }

  function onMessage(data: unknown, authSentAt: number): void {
    lastHeard = now();
    let frame: RawFrame;
    try {
      const parsed: unknown = JSON.parse(String(data));
      if (typeof parsed !== 'object' || parsed === null) return;
      frame = parsed as RawFrame;
    } catch {
      return;
    }
    if (frame.op === 'ready') {
      onReady(readyOf(frame), authSentAt);
      return;
    }
    if (frame.op === 'pong') {
      const sentAt = typeof frame.n === 'number' ? pings.get(frame.n) : undefined;
      if (sentAt !== undefined && typeof frame.n === 'number') {
        pings.delete(frame.n);
        measured(sentAt);
      }
      return;
    }
    if (!isFeedFrame(frame)) return;
    const sub = subs.get(frame.id);
    if (sub === undefined) return;
    // Permission and grammar refusals end the request; capacity waits for another view to leave.
    const waitsForCapacity = frame.op === 'error' && frame.code === 'too_many_subscriptions';
    if (waitsForCapacity) waitingCapacity.add(sub);
    else if (frame.op === 'closed' || frame.op === 'error') {
      waitingCapacity.delete(sub);
      subs.delete(frame.id);
    }
    if (frame.op === 'error' && (frame.code === 'forbidden' || frame.code === 'not_found')) refused.add(sub);
    sub.listener.onFrame(frame);
    if (frame.op === 'closed') fillReleasedSlot();
  }

  function onClose(code: number): void {
    socket = null;
    lost();
    if (!wanted) return;
    if (code === CLOSE_SUSPENDED) {
      wanted = false;
      status.setState({ phase: 'idle' });
      options.onSuspended();
      return;
    }
    if (code === CLOSE_OTHER_BUILD) {
      // A newer build is deployed: this page cannot speak to it, and says so instead of trying again.
      wanted = false;
      status.setState({ phase: 'idle', outdated: true });
      return;
    }
    if (code === CLOSE_UNAUTHENTICATED) {
      // The token is no longer good: a new one first, then the connection.
      void options
        .renew()
        .catch(() => true)
        .then((signedIn) => {
          if (signedIn) retryLater();
          else wanted = false;
        });
      return;
    }
    retryLater();
  }

  function open(): void {
    clearTimeout(retryTimer);
    if (!wanted || socket !== null) return;
    const token = options.token();
    if (token === null) {
      // Nobody is signed in on this page: there is nothing to connect as.
      status.setState({ phase: 'idle' });
      return;
    }
    status.setState({ phase: 'connecting' });
    let opened: SocketLike;
    try {
      opened = connect((options.url ?? defaultUrl)(), SUBPROTOCOL);
    } catch {
      lost();
      retryLater();
      return;
    }
    socket = opened;
    let authSentAt = now();
    opened.onopen = () => {
      if (socket !== opened) return;
      lastHeard = now();
      authSentAt = now();
      status.setState({ phase: 'authenticating' });
      opened.send(JSON.stringify({ op: 'auth', token: options.token() ?? token } satisfies ClientFrame));
    };
    opened.onmessage = (event) => {
      if (socket === opened) onMessage(event.data, authSentAt);
    };
    opened.onerror = () => undefined;
    opened.onclose = (event) => {
      if (socket === opened) onClose(event.code);
    };
  }

  return {
    status,

    start() {
      wanted = true;
      if (socket !== null) return;
      attempts = 0;
      open();
    },

    stop() {
      wanted = false;
      clearTimeout(retryTimer);
      clearInterval(heartbeatTimer);
      subs.clear();
      refused.clear();
      waitingCapacity.clear();
      const closing = socket;
      socket = null;
      closing?.close(1000);
      roundTrips = [];
      status.setState({ phase: 'idle', lostAt: null, roundTripMs: null });
    },

    subscribe(feed, listener) {
      listener = { ...listener, openingId: listener.openingId ?? openingId() };
      let id = nextSub;
      nextSub += 1;
      const sub = { feed, listener };
      subs.set(id, sub);
      if (status.getState().phase === 'ready') sendSub(id, feed, listener);
      return {
        unsubscribe() {
          refused.delete(sub);
          const wasWaiting = waitingCapacity.delete(sub);
          // After a reconnect the subscription has another id: find it by what it is.
          for (const [held, entry] of subs) if (entry === sub) id = held;
          if (subs.get(id) !== sub) return;
          subs.delete(id);
          send({ op: 'unsub', id });
          // Frames on one connection are ordered: the server handles this release before the retry.
          if (!wasWaiting) fillReleasedSlot();
        },
      };
    },

    visible(feed, visible) {
      send({ op: 'visible', feed, visible });
    },

    typing(feed, at) {
      send({ op: 'typing', feed, ...(at === undefined ? {} : { at }) });
    },

    reauth() {
      const token = options.token();
      if (token !== null && status.getState().phase === 'ready') send({ op: 'reauth', token });
    },

    retryRefused() {
      const waiting = [...refused];
      refused.clear();
      for (const sub of waiting) {
        const id = nextSub;
        nextSub += 1;
        subs.set(id, sub);
        if (status.getState().phase === 'ready') sendSub(id, sub.feed, sub.listener);
      }
    },

    wake() {
      if (!wanted || socket !== null) return;
      attempts = 0;
      open();
    },
  };
}
