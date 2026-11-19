/** Chat live state; the socket client owns live updates.
 *
 * Two things are kept here, apart from every thread. The first is a chat that is on screen: its
 * messages by the chat's own sequence, who is looking at it and who is typing, and the messages this
 * page sent that the server has not yet confirmed. A row is replaced only by one that changed later;
 * a message is never removed on the page's own initiative (a deleted one arrives as a row that says
 * so). The second is the person's own standing in every chat they have a state in, and the newest
 * sequence they have heard of for each, so that "unread" is known without a chat being open.
 *
 * Components read through the hooks near the end. The writers are for the live connection and for
 * the acts of the person; the rules a row obeys are plain functions, which the unit tests hold.
 */
import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { create } from 'zustand';

import { ApiError, client } from '@/api/client';
import { chatStatesKey, fetchChatStates, fetchChatSync } from '@/api/chats';
import { queryClient } from '@/api/query';
import type { Chat, ChatDraft, ChatMentionName, ChatMessage, ChatMessageListReply, ChatPart, ChatState, PresenceUser, UserRef } from '@/generated/api-types';
import { feeds, socket, useUserFeed } from '@/realtime/live';
import { onUserEvent } from '@/realtime/stores/user';
import { type DurableFeed, followFeed, loadWithRetries } from '@/realtime/sync';

// ---------------------------------------------------------------- the numbers of the protocol

/** How long a person is shown as typing after the last sign of it, as the protocol says. */
export const TYPING_SHOWN_MS = 6_000;
/** How often the person's own typing is told: the server lets one frame in three seconds through. */
export const TYPING_TELL_EVERY_MS = 3_000;

// ---------------------------------------------------------------- what is kept

/** A message this page sent and the server has not confirmed: in flight, or given up on. */
export interface Outgoing {
  /** The reviewed ordinary input substance is retried with this same acceptance key. */
  agentReview?: string;
  /** Local display labels kept with a failed or pending typed send; the server resolves identities. */
  mentionNames?: ChatMentionName[];
  /** The key of the act: the same for every attempt, so that a second one makes nothing twice. */
  clientId: string;
  parts: ChatPart[];
  replyTo: string | null;
  /** When the person pressed send, on this device's clock. */
  at: number;
  state: 'sending' | 'failed';
  /** What the server refused with, for a message that failed; `null` when the line was lost. */
  error: unknown;
}

export interface TypingMark {
  user: UserRef;
  /** When the last sign of it arrived, on this device's clock. */
  at: number;
}

export interface ChatEntry {
  /** The chat as the caller reads it; `null` until it arrived. */
  chat: Chat | null;
  /** The chat sequence the rows are as of. */
  lastSeq: number;
  /** `tail`: the newest messages and everything since. `window`: messages around one, with newer ones still to load. */
  loaded: 'none' | 'tail' | 'window';
  /** Every message the page holds, deleted ones included, by id. */
  messages: ReadonlyMap<string, ChatMessage>;
  /** The ids of those messages, oldest first. */
  order: readonly string[];
  hasOlder: boolean;
  /** Only in a window: whether messages newer than the last one held exist. */
  hasNewer: boolean;
  presence: readonly PresenceUser[];
  typing: ReadonlyMap<string, TypingMark>;
  outbox: readonly Outgoing[];
  /** What the last attempt to load it failed with, while nothing newer has succeeded. */
  error: unknown;
  /** Set when the server ended the subscription: the reader may no longer see the chat, or it is gone. */
  closed: 'revoked' | 'deleted' | null;
}

/** What a load or a frame brings. */
export interface ChatRows {
  chat?: Chat | null;
  messages?: readonly ChatMessage[] | null;
}

const NO_MESSAGES: ReadonlyMap<string, ChatMessage> = new Map();
const NO_TYPING: ReadonlyMap<string, TypingMark> = new Map();

export const EMPTY_ENTRY: ChatEntry = {
  chat: null,
  lastSeq: 0,
  loaded: 'none',
  messages: NO_MESSAGES,
  order: [],
  hasOlder: false,
  hasNewer: false,
  presence: [],
  typing: NO_TYPING,
  outbox: [],
  error: null,
  closed: null,
};

// ---------------------------------------------------------------- the rules a row obeys (pure)

/** The ids of `messages` laid into `order`, which is oldest first by the sequence a message was written at. */
export function insertOrdered(
  order: readonly string[],
  held: ReadonlyMap<string, ChatMessage>,
  incoming: readonly ChatMessage[],
): string[] {
  const seen = new Set(order);
  const fresh = incoming.filter((message, at) => !seen.has(message.id) && incoming.findIndex((other) => other.id === message.id) === at);
  if (fresh.length === 0) return [...order];
  const merged = [...order, ...fresh.map((message) => message.id)];
  const created = (id: string): number => held.get(id)?.seq_created ?? Number.MAX_SAFE_INTEGER;
  // The common case, a message newer than every one held, keeps the order as it is.
  return merged.sort((one, other) => created(one) - created(other));
}

/** The highest chat sequence a message of `entry` was written at; zero for an empty chat. */
export function newestHeld(entry: Pick<ChatEntry, 'order' | 'messages'>): number {
  const last = entry.order.at(-1);
  return last === undefined ? 0 : (entry.messages.get(last)?.seq_created ?? 0);
}

/**
 * A load that failed. When the server answers that the chat can no longer be read by someone who had read it
 * (they left the group, lost the right, or the chat is gone), the reading is over, as if the server had said
 * so on the feed: what was on screen is not left standing. Any other failure leaves the rows and notes the error.
 */
export function reduceFailed(entry: ChatEntry, error: unknown): ChatEntry {
  const held = entry.chat;
  if (held !== null && error instanceof ApiError && (error.status === 403 || error.status === 404)) {
    return { ...entry, error, closed: error.status === 404 && held.kind === 'node' ? 'deleted' : 'revoked' };
  }
  return { ...entry, error };
}

/** A whole load: the chat and its newest messages, in place of what was held. */
export function reduceReset(entry: ChatEntry, chat: Chat, messages: readonly ChatMessage[], hasOlder: boolean, seq: number): ChatEntry {
  const held = new Map(messages.map((message) => [message.id, message]));
  return {
    ...entry,
    chat,
    lastSeq: seq,
    loaded: 'tail',
    messages: held,
    order: insertOrdered([], held, messages),
    hasOlder,
    hasNewer: false,
    error: null,
    closed: null,
  };
}

/**
 * Rows that changed. A message replaces the one held only when it changed later; a new message is laid
 * into its place, unless the page holds a window of the past and the message lies beyond it, in which
 * case it is left for when the reader goes there. Whoever wrote a new message has stopped typing.
 */
export function reduceRows(entry: ChatEntry, rows: ChatRows, seq: number): ChatEntry {
  let messages: Map<string, ChatMessage> | null = null;
  const added: ChatMessage[] = [];
  const edge = newestHeld(entry);
  let hasNewer = entry.hasNewer;
  for (const row of rows.messages ?? []) {
    const held = (messages ?? entry.messages).get(row.id);
    if (held !== undefined) {
      if (held.seq_updated >= row.seq_updated) continue;
      messages ??= new Map(entry.messages);
      messages.set(row.id, row);
      continue;
    }
    if (entry.loaded === 'window' && row.seq_created > edge) {
      hasNewer = true;
      continue;
    }
    messages ??= new Map(entry.messages);
    messages.set(row.id, row);
    added.push(row);
  }
  const incomingChat = rows.chat ?? null;
  const chat = incomingChat !== null && (entry.chat === null || incomingChat.seq >= entry.chat.seq) ? incomingChat : entry.chat;
  if (messages === null && chat === entry.chat && seq <= entry.lastSeq && hasNewer === entry.hasNewer) return entry;

  let typing = entry.typing;
  for (const message of added) {
    const author = message.author;
    if (author !== null && 'id' in author && typing.has(author.id)) {
      const next = new Map(typing);
      next.delete(author.id);
      typing = next;
    }
  }
  const held = messages ?? entry.messages;
  return {
    ...entry,
    chat,
    lastSeq: Math.max(entry.lastSeq, seq),
    messages: held,
    order: added.length === 0 ? entry.order : insertOrdered(entry.order, held, added),
    hasNewer,
    typing,
    error: null,
  };
}

/** Older messages, read when the reader came to the top of what is held. */
export function reduceOlder(entry: ChatEntry, reply: Pick<ChatMessageListReply, 'messages' | 'has_older'>): ChatEntry {
  const held = new Map(entry.messages);
  for (const message of reply.messages) {
    const own = held.get(message.id);
    if (own === undefined || own.seq_updated < message.seq_updated) held.set(message.id, message);
  }
  return { ...entry, messages: held, order: insertOrdered(entry.order, held, reply.messages), hasOlder: reply.has_older };
}

/** Messages around one, in place of what was held: the page now shows a window of the past. */
export function reduceAround(entry: ChatEntry, reply: ChatMessageListReply): ChatEntry {
  const held = new Map(reply.messages.map((message) => [message.id, message]));
  return {
    ...entry,
    messages: held,
    order: insertOrdered([], held, reply.messages),
    hasOlder: reply.has_older,
    hasNewer: reply.has_newer,
    loaded: reply.has_newer ? 'window' : 'tail',
  };
}

/** Newer messages, read when the reader came to the end of a window. */
export function reduceNewer(entry: ChatEntry, reply: Pick<ChatMessageListReply, 'messages' | 'has_newer'>): ChatEntry {
  const held = new Map(entry.messages);
  for (const message of reply.messages) {
    const own = held.get(message.id);
    if (own === undefined || own.seq_updated < message.seq_updated) held.set(message.id, message);
  }
  return {
    ...entry,
    messages: held,
    order: insertOrdered(entry.order, held, reply.messages),
    hasNewer: reply.has_newer,
    loaded: reply.has_newer ? 'window' : 'tail',
  };
}

/** Who is typing now: the marks that are younger than the protocol's six seconds, oldest first. */
export function typingNow(typing: ReadonlyMap<string, TypingMark>, now: number, except: string | null): UserRef[] {
  return [...typing.values()]
    .filter((mark) => now - mark.at < TYPING_SHOWN_MS && mark.user.id !== except)
    .sort((one, other) => one.at - other.at)
    .map((mark) => mark.user);
}

/** Whether a chat is unread for a person: they follow it, and the newest message is beyond what they read. */
export function isUnread(state: Pick<ChatState, 'effective_level' | 'last_read_seq'> | null | undefined, newestSeq: number): boolean {
  return state !== null && state !== undefined && state.effective_level !== 'off' && newestSeq > state.last_read_seq;
}

// ---------------------------------------------------------------- the store

interface ChatStore {
  byChat: ReadonlyMap<string, ChatEntry>;
}

export const useChatStore = create<ChatStore>(() => ({ byChat: new Map() }));

function update(chatId: string, change: (entry: ChatEntry) => ChatEntry): void {
  useChatStore.setState((state) => {
    const before = state.byChat.get(chatId) ?? EMPTY_ENTRY;
    const after = change(before);
    if (after === before) return state;
    const byChat = new Map(state.byChat);
    byChat.set(chatId, after);
    return { byChat };
  });
}

/** The writers. The live connection and the acts of the person call them; no view does. */
export const chatWriter = {
  reset(chatId: string, chat: Chat, messages: readonly ChatMessage[], hasOlder: boolean, seq: number): void {
    update(chatId, (entry) => reduceReset(entry, chat, messages, hasOlder, seq));
    acceptStanding(chatId, { chat, messages });
  },

  apply(chatId: string, rows: ChatRows, seq: number): void {
    update(chatId, (entry) => reduceRows(entry, rows, seq));
    acceptStanding(chatId, rows);
  },

  /** What the person's own request answered, taken like a row of the feed. */
  own(chatId: string, message: ChatMessage): void {
    update(chatId, (entry) => reduceRows(entry, { messages: [message] }, entry.lastSeq));
    acceptStanding(chatId, { messages: [message] });
  },

  older(chatId: string, reply: Pick<ChatMessageListReply, 'messages' | 'has_older'>): void {
    update(chatId, (entry) => reduceOlder(entry, reply));
  },

  around(chatId: string, reply: ChatMessageListReply): void {
    update(chatId, (entry) => reduceAround(entry, reply));
  },

  newer(chatId: string, reply: Pick<ChatMessageListReply, 'messages' | 'has_newer'>): void {
    update(chatId, (entry) => reduceNewer(entry, reply));
  },

  /** A row as the person's own act shows it before the server answers: it takes the place of the one held, whatever its stamp. */
  shown(chatId: string, message: ChatMessage): void {
    update(chatId, (entry) => (entry.messages.has(message.id) ? { ...entry, messages: new Map(entry.messages).set(message.id, message) } : entry));
  },

  /** Puts back what was shown before, unless another row has taken its place since. */
  unshown(chatId: string, shown: ChatMessage, original: ChatMessage): void {
    update(chatId, (entry) => (entry.messages.get(shown.id) === shown ? { ...entry, messages: new Map(entry.messages).set(original.id, original) } : entry));
  },

  presence(chatId: string, users: readonly PresenceUser[]): void {
    update(chatId, (entry) => ({ ...entry, presence: users }));
  },

  typed(chatId: string, user: UserRef, now: number): void {
    update(chatId, (entry) => {
      const typing = new Map(entry.typing);
      for (const [id, mark] of typing) if (now - mark.at >= TYPING_SHOWN_MS) typing.delete(id);
      typing.set(user.id, { user, at: now });
      return { ...entry, typing };
    });
  },

  failed(chatId: string, error: unknown): void {
    update(chatId, (entry) => reduceFailed(entry, error));
  },

  closed(chatId: string, reason: 'revoked' | 'deleted'): void {
    update(chatId, (entry) => ({ ...entry, closed: reason }));
  },

  // A message on its way out.
  outgoing(chatId: string, sent: Outgoing): void {
    update(chatId, (entry) => ({ ...entry, outbox: [...entry.outbox.filter((one) => one.clientId !== sent.clientId), sent] }));
  },
  outgoingChanged(chatId: string, clientId: string, change: Partial<Pick<Outgoing, 'state' | 'error'>>): void {
    update(chatId, (entry) => ({
      ...entry,
      outbox: entry.outbox.map((one) => (one.clientId === clientId ? { ...one, ...change } : one)),
    }));
  },
  outgoingDone(chatId: string, clientId: string): void {
    update(chatId, (entry) => ({ ...entry, outbox: entry.outbox.filter((one) => one.clientId !== clientId) }));
  },

  /** Forgets a chat nobody shows any more. */
  evict(chatId: string): void {
    useChatStore.setState((state) => {
      const held = state.byChat.get(chatId);
      if (held === undefined) return state;
      const byChat = new Map(state.byChat);
      // A message that was not sent stays with its text: it is the person's, and not a copy of the server's rows.
      if (held.outbox.length > 0) byChat.set(chatId, { ...EMPTY_ENTRY, outbox: held.outbox });
      else byChat.delete(chatId);
      return { byChat };
    });
  },

  clear(): void {
    useChatStore.setState({ byChat: new Map() });
  },
};

// ---------------------------------------------------------------- the person's standing in every chat

interface Latest {
  /** The newest chat sequence heard of, from a `chat` event of the person's own feed or from a list. */
  seq: number;
  at: string | null;
}

interface StatesStore {
  /** The caller's state in each chat where they have one. */
  states: ReadonlyMap<string, ChatState>;
  /** The newest message heard of in each direct or group chat, by chat. */
  latest: ReadonlyMap<string, Latest>;
  loaded: boolean;
}

export const useChatStates = create<StatesStore>(() => ({ states: new Map(), latest: new Map(), loaded: false }));

/** Within one observed revision reads advance; a newer deliberate revision may move backwards. */
export function mergeState(held: ChatState | undefined, incoming: ChatState): ChatState {
  if (held === undefined) return incoming;
  if (incoming.read_revision > held.read_revision) return incoming;
  if (incoming.read_revision < held.read_revision) return held;
  const lastRead = Math.max(held.last_read_seq, incoming.last_read_seq);
  // Mentions counted before the position the person has read to are read.
  const mentions = incoming.last_read_seq >= held.last_read_seq ? incoming.mentions_unread : Math.min(incoming.mentions_unread, held.mentions_unread);
  return { ...incoming, last_read_seq: lastRead, mentions_unread: mentions };
}

export const statesWriter = {
  loaded(list: readonly ChatState[]): void {
    useChatStates.setState((current) => {
      const states = new Map<string, ChatState>();
      for (const state of list) states.set(state.chat_id, mergeState(current.states.get(state.chat_id), state));
      return { ...current, states, loaded: true };
    });
  },

  put(state: ChatState): void {
    queryClient.setQueryData<ChatState[]>(chatStatesKey, (held) => {
      const rows = held ?? [];
      const before = rows.find((row) => row.chat_id === state.chat_id);
      const next = mergeState(before, state);
      return before === undefined ? [...rows, next] : rows.map((row) => row.chat_id === state.chat_id ? next : row);
    });
    useChatStates.setState((current) => {
      const states = new Map(current.states);
      states.set(state.chat_id, mergeState(current.states.get(state.chat_id), state));
      return { ...current, states };
    });
  },

  /** The person read a chat up to a sequence: shown at once, and put right if the server refuses. */
  read(chatId: string, seq: number, base: ChatState | null): void {
    useChatStates.setState((current) => {
      const held = current.states.get(chatId) ?? base;
      if (held === null || held.last_read_seq >= seq) return current;
      const states = new Map(current.states);
      states.set(chatId, { ...held, last_read_seq: seq, mentions_unread: 0 });
      return { ...current, states };
    });
  },

  heard(chatId: string, seq: number, at: string | null): void {
    useChatStates.setState((current) => {
      const before = current.latest.get(chatId);
      if (before !== undefined && before.seq >= seq) return current;
      const latest = new Map(current.latest);
      latest.set(chatId, { seq, at });
      return { ...current, latest };
    });
  },

  clear(): void {
    useChatStates.setState({ states: new Map(), latest: new Map(), loaded: false });
  },
};

/** The conversation and its surrounding indicators consume the same returned facts.
 * A node's tree can be hidden or cached; its chat feed still owns current message activity.
 * Only message creation advances this position, never a reaction/edit/frame sequence.
 */
function acceptStanding(chatId: string, rows: ChatRows): void {
  if (rows.chat?.my !== null && rows.chat?.my !== undefined) statesWriter.put(rows.chat.my);
  if (rows.chat !== null && rows.chat !== undefined) statesWriter.heard(chatId, rows.chat.last_message_seq, rows.chat.last_message_at);
  for (const message of rows.messages ?? []) {
    if (message.kind === 'message') statesWriter.heard(chatId, message.seq_created, message.created_at);
  }
}

client.onSession((event) => {
  if (event.type !== 'signed_out') return;
  // `feeds.clear()` evicts every chat that was followed; what only the person's own acts hold goes too.
  chatWriter.clear();
  statesWriter.clear();
});

// ---------------------------------------------------------------- the chat feed

const chatFeedKey = (chatId: string) => `chat:${chatId}`;

/** What a load of a chat, or a frame of its feed, brings. */
interface ChatFeedData extends ChatRows {
  hasOlder: boolean;
}

function chatFeed(chatId: string): DurableFeed<ChatFeedData> {
  return {
    feed: chatFeedKey(chatId),
    initial: () => {
      const held = useChatStore.getState().byChat.get(chatId);
      if (held?.chat === undefined || held.chat === null || held.loaded !== 'tail' || held.closed !== null) return null;
      return { seq: held.lastSeq, complete: true, data: { chat: held.chat, messages: [...held.messages.values()], hasOlder: held.hasOlder } };
    },
    invalidated: () => chatWriter.evict(chatId),
    load: async (since, signal) => {
      const reply = await loadWithRetries(() => fetchChatSync(chatId, since, signal), signal);
      return {
        seq: reply.seq,
        complete: reply.complete,
        data: { chat: reply.chat, messages: reply.messages, hasOlder: reply.has_older },
      };
    },
    reset: (data, seq) => {
      if (data.chat !== null && data.chat !== undefined) chatWriter.reset(chatId, data.chat, data.messages ?? [], data.hasOlder, seq);
    },
    apply: (data, seq) => chatWriter.apply(chatId, data, seq),
    read: (data) => ({
      chat: (data.chat ?? null) as Chat | null,
      messages: Array.isArray(data.messages) ? (data.messages as ChatMessage[]) : [],
      hasOlder: false,
    }),
    failed: (error) => chatWriter.failed(chatId, error),
    closed: (reason) => chatWriter.closed(chatId, reason),
    subscribed: () => tellVisible(chatId),
    other: (frame) => {
      if (frame.op === 'presence') chatWriter.presence(chatId, frame.users);
      else if (frame.op === 'event' && frame.data.what === 'typing' && isUserRef(frame.data.user)) {
        chatWriter.typed(chatId, frame.data.user, Date.now());
      }
    },
  };
}

function isUserRef(value: unknown): value is UserRef {
  return typeof value === 'object' && value !== null && typeof (value as { id?: unknown }).id === 'string';
}

/** The chats that a view shows at the moment, so that the server is told what is on screen. */
const shown = new Map<string, number>();

function tellVisible(chatId: string): void {
  const on = (shown.get(chatId) ?? 0) > 0 && typeof document !== 'undefined' && document.visibilityState === 'visible';
  socket.visible(chatFeedKey(chatId), on);
}

if (typeof document !== 'undefined') {
  // The tab is shown or hidden: the server counts who is looking by it.
  document.addEventListener('visibilitychange', () => {
    for (const chatId of shown.keys()) tellVisible(chatId);
  });
}

/**
 * Follows a chat for as long as the calling view is shown, and tells the server that it is on screen.
 * `watching` is false while the chat is in a tab nobody has in front: it is then not followed at all.
 */
export function useChatFeed(chatId: string | null, watching = true): void {
  useEffect(() => {
    if (chatId === null || !watching) return;
    const release = feeds.use(
      chatFeedKey(chatId),
      () => followFeed(socket, chatFeed(chatId)),
      () => chatWriter.evict(chatId),
    );
    shown.set(chatId, (shown.get(chatId) ?? 0) + 1);
    tellVisible(chatId);
    return () => {
      const left = (shown.get(chatId) ?? 1) - 1;
      if (left <= 0) shown.delete(chatId);
      else shown.set(chatId, left);
      socket.visible(chatFeedKey(chatId), left > 0);
      release();
    };
  }, [chatId, watching]);
}

/** Loads a chat again from nothing: after a failure, and to go back to the newest messages from a window of the past. */
export function reloadChat(chatId: string): void {
  feeds.reload(chatFeedKey(chatId));
}

/** Asks a followed chat for what changed since it was last read: after an act of the person's own. */
export function refreshChat(chatId: string): void {
  feeds.refresh(chatFeedKey(chatId));
}

/** Tells the server that the person is typing in a chat, at most as often as the protocol lets a frame through. */
const toldTyping = new Map<string, number>();
export function tellTyping(chatId: string, now: number = Date.now()): void {
  const before = toldTyping.get(chatId);
  if (before !== undefined && now - before < TYPING_TELL_EVERY_MS) return;
  toldTyping.set(chatId, now);
  socket.typing(chatFeedKey(chatId));
}

// ---------------------------------------------------------------- the person's own feed, for chats

type DraftListener = (chatId: string, draft: ChatDraft | null) => void;
const draftListeners = new Set<DraftListener>();

/** Hears of a draft that was changed on another of the person's devices. What it returns stops the listening. */
export function onChatDraftChanged(listener: DraftListener): () => void {
  draftListeners.add(listener);
  return () => draftListeners.delete(listener);
}

/** Reads the person's states again: after they wrote in a chat, which makes them follow it. */
export function refreshChatStates(): void {
  loadStates();
}

function loadStates(): void {
  const held = queryClient.getQueryData<ChatState[]>(chatStatesKey);
  if (held !== undefined) { statesWriter.loaded(held); return; }
  queryClient
    .query({ queryKey: chatStatesKey, queryFn: ({ signal }) => fetchChatStates(signal) })
    .then((list) => statesWriter.loaded(list))
    .catch(() => undefined);
}

/** A new message was written in a direct or group chat the person is in, or a chat of theirs began. */
function heardOfChat(chatId: string, seq: number, at: string): void {
  statesWriter.heard(chatId, seq, at);
  const found = { listed: false };
  queryClient.setQueriesData<{ pages: { items: Chat[]; next_cursor: string | null }[]; pageParams: unknown[] }>(
    { queryKey: ['chats'] },
    (data) => {
      if (data === undefined) return data;
      const pages = data.pages.map((page) => ({
        ...page,
        items: page.items.map((chat) => {
          if (chat.id !== chatId) return chat;
          found.listed = true;
          return chat.last_message_seq >= seq ? chat : { ...chat, last_message_seq: seq, last_message_at: at };
        }),
      }));
      return { ...data, pages };
    },
  );
  // A chat that no list holds is a new one, or one that was hidden and has a message again.
  if (!found.listed) void queryClient.invalidateQueries({ queryKey: ['chats'] });
}

function onUserFrame(frame: Parameters<NonNullable<DurableFeed<unknown>['other']>>[0] | { op: string }): void {
  if (frame.op === 'subscribed' || frame.op === 'resync') {
    return;
  }
  if (frame.op !== 'event' || !('data' in frame)) return;
  const data = frame.data;
  switch (data.what) {
    case 'chat_state':
      if (typeof data.state === 'object' && data.state !== null) statesWriter.put(data.state as ChatState);
      return;
    case 'chat_draft':
      if (typeof data.chat_id === 'string') for (const listener of draftListeners) listener(data.chat_id, (data.draft ?? null) as ChatDraft | null);
      return;
    case 'chat':
      if (typeof data.chat_id === 'string' && typeof data.seq === 'number') {
        heardOfChat(data.chat_id, data.seq, typeof data.at === 'string' ? data.at : new Date().toISOString());
      }
      return;
    default:
  }
}

/**
 * Keeps the person's standing in every chat current for as long as somebody is signed in: reads the
 * states, and listens for the events of their own feed that concern chats. The shell mounts it once.
 */
export function useChatActivity(): void {
  useUserFeed(false);
  const signedIn = client.session.getState().status === 'signed_in';
  useEffect(() => {
    if (!signedIn) return;
    loadStates();
    const stop = ['chat_state', 'chat_draft', 'chat'].map((what) => onUserEvent(what, (data) => onUserFrame({ op: 'event', id: 0, feed: 'user', data })));
    return () => stop.forEach((off) => off());
  }, [signedIn]);
}

// ---------------------------------------------------------------- reading

/** A chat as far as it has arrived, and the empty entry before it has. */
export function useChatEntry(chatId: string | null): ChatEntry {
  return useChatStore((state) => (chatId === null ? EMPTY_ENTRY : (state.byChat.get(chatId) ?? EMPTY_ENTRY)));
}

/** The caller's state in a chat: what the person's own feed says, else what the chat said when it was read. */
export function useChatState(chatId: string | null, fallback: ChatState | null = null): ChatState | null {
  return useChatStates((state) => (chatId === null ? null : (state.states.get(chatId) ?? fallback)));
}

/** The newest message heard of in a direct or group chat. */
export function useLatestSeq(chatId: string | null): number {
  return useChatStates((state) => (chatId === null ? 0 : (state.latest.get(chatId)?.seq ?? 0)));
}

/** Whether a node's chat, given the sequence its tree row says it has come to, has something the person has not read. */
export function useChatUnread(chatId: string | null, newestSeq: number, fallback: ChatState | null = null): { unread: boolean; mentions: number; state: ChatState | null } {
  const state = useChatState(chatId, fallback);
  const heard = useLatestSeq(chatId);
  return useMemo(
    () => ({ unread: isUnread(state, Math.max(newestSeq, heard)), mentions: state?.mentions_unread ?? 0, state }),
    [state, newestSeq, heard],
  );
}

/** Who is typing in a chat now, which the view asks again every second while anyone is shown. */
export function useTyping(typing: ReadonlyMap<string, TypingMark>, except: string | null): UserRef[] {
  const stamp = useTick(typing.size > 0);
  return useMemo(() => typingNow(typing, stamp, except), [typing, stamp, except]);
}

/** One clock for everything that expires, ticking once a second while anything listens. */
const tickers = new Set<() => void>();
let tickAt = Date.now();
let tickTimer: ReturnType<typeof setInterval> | undefined;

function listenToTicks(listener: () => void): () => void {
  tickers.add(listener);
  if (tickers.size === 1) {
    tickAt = Date.now();
    tickTimer = setInterval(() => {
      tickAt = Date.now();
      for (const one of tickers) one();
    }, 1_000);
  }
  return () => {
    tickers.delete(listener);
    if (tickers.size === 0) clearInterval(tickTimer);
  };
}

const listenToNothing = (): (() => void) => () => undefined;

/** The time of the last tick of a clock that runs only while `running`. */
function useTick(running: boolean): number {
  return useSyncExternalStore(running ? listenToTicks : listenToNothing, () => tickAt);
}
