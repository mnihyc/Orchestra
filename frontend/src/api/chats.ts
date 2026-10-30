/** API client for Chats, messages, forwarding, and sharing with agents and taking it back.
 *
 * What the chat screens ask of the API: the chats of a root org and of one node, a chat with its latest
 * messages or what changed in it, older messages and messages around one, and the acts that write in a
 * chat: sending, editing and deleting, reacting, pinning, reading, the bell, a draft, the people of a
 * group, and forwarding copies of messages. A message and a forward carry the key of the act in their
 * body, so that a second attempt after a lost answer makes nothing twice; a group made carries it as
 * the header every creating request has. Sharing a chat with agents is not served yet and is not here.
 */
import { infiniteQueryOptions, queryOptions, type QueryClient } from '@tanstack/react-query';

import { api, client, done, ok, repeatable } from '@/api/client';
import { queryClient } from '@/api/query';
import { beginPeopleRead } from '@/api/peopleChanges';
import { chatWriter, useChatStore } from '@/realtime/stores/chat';
import type {
  Chat,
  ChatDraft,
  ChatDraftSetBody,
  ChatLevel,
  ChatListScope,
  ChatMessage,
  ChatMessageCreateBody,
  ChatMessageCreateReply,
  ChatMessageListReply,
  ChatPage,
  ChatPart,
  ChatState,
  ChatSync,
  ChatOpening,
  NodeChatOpening,
  NodeKind,
  ForwardCreateBody,
  ForwardCreateReply,
  Item,
  OrgChatCreateBody,
  UserRef,
} from '@/generated/api-types';

// ---------------------------------------------------------------- parts

/** A copy made by forwarding. On the wire it says `type: 'forwarded'`, which the generated types leave out of it. */
export type ForwardedPart = Extract<ChatPart, { type: 'forwarded' }>;

// ---------------------------------------------------------------- the numbers the contract states

/** How many messages a page of a chat holds when it is asked for without a number: the contract's own. */
export const MESSAGES_PAGE = 100;
/** The longest name a group can have, in characters. */
export const GROUP_NAME_MAX_CHARS = 80;
/** The most parts one message holds. */
export const MESSAGE_MAX_PARTS = 32;
/** The most messages of one chat a forward takes, and the most destinations it writes to. */
export const FORWARD_MAX_MESSAGES = 20;
/** The most items of a thread a forward takes. */
export const FORWARD_MAX_ITEMS = 200;
export const FORWARD_MAX_DESTINATIONS = 10;
/** The fewest characters a search for people is asked with. */
export const PEOPLE_SEARCH_MIN_CHARS = 2;
/** How many people a search for people shows: more than a list to choose from is read as a result to search again. */
export const PEOPLE_SHOWN = 8;
/** How many chats a page of a list asks for. */
export const CHATS_PAGE = 50;

// ---------------------------------------------------------------- reading

export const chatKey = (chatId: string) => ['chat', chatId] as const;

/** A chat as the caller reads it, without its messages. */
export const chatQuery = (chatId: string) =>
  queryOptions({
    queryKey: chatKey(chatId),
    queryFn: ({ signal }): Promise<Chat> => ok(api.GET('/chats/{id}', { params: { path: { id: chatId } }, signal })),
  });

/** The chat and its latest messages, or what changed after a chat sequence: how the live feed loads and repairs itself. */
export function fetchChatSync(chatId: string, since: number | null, signal?: AbortSignal): Promise<ChatSync> {
  if (since === null) {
    const reader = client.session.getState().readerVersion;
    return ok(api.GET('/chats/{id}/opening', { params: { path: { id: chatId }, query: { limit: MESSAGES_PAGE } }, signal })).then((opening) => {
      signal?.throwIfAborted();
      if (client.session.getState().readerVersion !== reader) throw new DOMException('The chat reader changed', 'AbortError');
      seedChatDraft(queryClient, opening);
      return opening.sync;
    });
  }
  return ok(
    api.GET('/chats/{id}/sync', {
      params: { path: { id: chatId }, query: { limit: MESSAGES_PAGE, since } },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

function seedChatDraft(cache: QueryClient, opening: ChatOpening): void {
  const key = draftKey(opening.sync.chat.id);
  // A reopened or expanded composer must not overwrite a locally edited draft.
  if (cache.getQueryState(key) === undefined) cache.setQueryData(key, opening.draft);
}

/** One independently admitted panel opening supplies scopes and the selected conversation. */
export const nodeChatOpeningQuery = (nodeId: string, scope: string, chatId: string | null) => queryOptions({
  queryKey: ['node-chat-opening', nodeId] as const,
  queryFn: async ({ signal, client: cache, queryKey }): Promise<NodeChatOpening> => {
    const checked = beginPeopleRead(cache, queryKey);
    try {
    const reader = client.session.getState().readerVersion;
    const preferred: NodeKind | undefined = ['org', 'project', 'topic', 'thread'].includes(scope) ? scope as NodeKind : undefined;
    const data = await ok(api.GET('/nodes/{id}/chat-opening', {
      params: { path: { id: nodeId }, query: { scope: preferred, ...(chatId === null ? {} : { chat_id: chatId }), limit: MESSAGES_PAGE } }, signal,
    }));
    signal.throwIfAborted();
    if (client.session.getState().readerVersion !== reader) throw new DOMException('The chat reader changed', 'AbortError');
    if (data.opening !== null) {
      seedChatDraft(cache, data.opening);
      const sync = data.opening.sync;
      const held = useChatStore.getState().byChat.get(sync.chat.id);
      // A simultaneous live frame may already have advanced the admitted conversation.
      if (held === undefined || held.lastSeq <= sync.seq) chatWriter.reset(sync.chat.id, sync.chat, sync.messages, sync.has_older, sync.seq);
    }
    checked.accept();
    return data;
    } catch (error) { checked.cancel(); throw error; }
  },
  retry: false,
});

/** Which messages of a chat to read; exactly one way is given. */
export type MessageWindow =
  /** The page before a chat sequence. */
  | { before: number }
  /** The page after a chat sequence. */
  | { after: number }
  /** The page around one message. */
  | { around: string }
  /** The pinned messages. */
  | { pinned: true }
  /** The messages that quote an item of the thread whose chat this is. */
  | { item: string };

export function fetchMessages(
  chatId: string,
  window: MessageWindow,
  signal?: AbortSignal,
  limit: number = MESSAGES_PAGE,
): Promise<ChatMessageListReply> {
  const query =
    'before' in window
      ? { before: window.before }
      : 'after' in window
        ? { after: window.after }
        : 'around' in window
          ? { around: window.around }
          : 'pinned' in window
            ? { pinned: true }
            : { item_id: window.item };
  return ok(
    api.GET('/chats/{id}/messages', {
      params: { path: { id: chatId }, query: { ...query, limit } },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** The pinned messages of a chat, oldest first. */
export const pinnedKey = (chatId: string) => ['chat', chatId, 'pinned'] as const;
export const pinnedQuery = (chatId: string) =>
  queryOptions({
    queryKey: pinnedKey(chatId),
    queryFn: ({ signal }): Promise<ChatMessage[]> => fetchMessages(chatId, { pinned: true }, signal).then((reply) => reply.messages),
  });

export const chatStatesKey = ['chat-states'] as const;

/** The caller's state in every chat where they have one: how far they read, their bell, whether they hid it. */
export function fetchChatStates(signal?: AbortSignal): Promise<ChatState[]> {
  return ok(api.GET('/me/chat-states', signal === undefined ? {} : { signal }));
}

export const orgChatsKey = (orgId: string) => ['chats', orgId] as const;

/** What a list of an org's chats is of: the caller's own, the others' they may read, or those of nodes. */
export interface ChatListFilter {
  participating?: boolean;
  scope: ChatListScope;
  /** With `others`: the org of the tree whose people's chats are read. */
  of: string | null;
}

/** The chats of a root org, newest activity first, a page at a time. */
export const orgChatsQuery = (orgId: string, filter: ChatListFilter) =>
  infiniteQueryOptions({
    queryKey: [...orgChatsKey(orgId), filter] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<ChatPage> =>
      ok(
        api.GET('/orgs/{id}/chats', {
          params: {
            path: { id: orgId },
            query: {
              scope: filter.scope,
              ...(filter.participating === true ? { participating: true } : {}),
              limit: CHATS_PAGE,
              ...(filter.of === null ? {} : { of: filter.of }),
              ...(pageParam === null ? {} : { cursor: pageParam }),
            },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
    // A list the caller may not read (the others' chats without the permission) is an answer, not a fault to repeat.
    retry: false,
  });

/** Searches the people the caller shares an org with, by the start of a username or a part of a name. */
export const peopleKey = (orgId: string | null, q: string) => ['people', orgId, q] as const;
export const peopleQuery = (orgId: string | null, q: string) =>
  queryOptions({
    queryKey: peopleKey(orgId, q),
    queryFn: ({ signal }): Promise<UserRef[]> =>
      ok(api.GET('/users/search', { params: { query: { q, ...(orgId === null ? {} : { org_id: orgId }) } }, signal })),
    staleTime: 60_000,
  });

/** An item of a thread, as a quote shows it. A reader who may not read the timeline is refused, and that is final. */
export const quotedItemKey = (threadId: string, itemId: string) => ['thread-item', threadId, itemId] as const;
export const quotedItemQuery = (threadId: string, itemId: string) =>
  queryOptions({
    queryKey: quotedItemKey(threadId, itemId),
    queryFn: ({ signal }): Promise<Item> =>
      ok(api.GET('/threads/{id}/items/{item_id}', { params: { path: { id: threadId, item_id: itemId } }, signal })),
    retry: false,
    staleTime: 5 * 60_000,
  });

// ---------------------------------------------------------------- writing in a chat

/** Sends a message. `body.client_id` is made when the person acts and is the same for every attempt. */
export function sendMessage(chatId: string, body: ChatMessageCreateBody): Promise<ChatMessageCreateReply> {
  return ok(api.POST('/chats/{id}/messages', { params: { path: { id: chatId } }, body }));
}

export function editMessage(chatId: string, messageId: string, parts: ChatPart[]): Promise<ChatMessage> {
  return ok(
    api.PATCH('/chats/{id}/messages/{message_id}', { params: { path: { id: chatId, message_id: messageId } }, body: { parts } }),
  );
}

/** Deletes a message: the row stays, with `deleted` set and nothing in it. */
export function deleteMessage(chatId: string, messageId: string): Promise<ChatMessage> {
  return ok(api.DELETE('/chats/{id}/messages/{message_id}', { params: { path: { id: chatId, message_id: messageId } } }));
}

/** Adds or takes away the caller's own reaction. */
export function react(chatId: string, messageId: string, emoji: string, on: boolean): Promise<ChatMessage> {
  const params = { path: { id: chatId, message_id: messageId, emoji } };
  return ok(
    on
      ? api.PUT('/chats/{id}/messages/{message_id}/reactions/{emoji}', { params })
      : api.DELETE('/chats/{id}/messages/{message_id}/reactions/{emoji}', { params }),
  );
}

export function pin(chatId: string, messageId: string, on: boolean): Promise<ChatMessage> {
  const params = { path: { id: chatId, message_id: messageId } };
  return ok(on ? api.PUT('/chats/{id}/messages/{message_id}/pin', { params }) : api.DELETE('/chats/{id}/messages/{message_id}/pin', { params }));
}

/** Says how far the caller has read; the stored position only moves forward. */
export function markRead(chatId: string, seq: number, readRevision: number): Promise<void> {
  return done(api.POST('/chats/{id}/read', { params: { path: { id: chatId } }, body: { seq, read_revision: readRevision } }));
}

/** Marks every chat of the org that the caller follows as read up to now. */
export function markOrgRead(orgId: string): Promise<void> {
  return done(api.POST('/orgs/{id}/chats/read', { params: { path: { id: orgId } } }));
}

/** The bell and the hiding of a chat; what is left out stays, and `level: null` returns to the default. */
export function setChatState(chatId: string, change: { level?: ChatLevel | null; hidden?: boolean }): Promise<ChatState> {
  return ok(api.PUT('/chats/{id}/state', { params: { path: { id: chatId } }, body: change }));
}

// ---------------------------------------------------------------- drafts

export const draftKey = (chatId: string) => ['chat', chatId, 'draft'] as const;

/** The caller's draft in a chat, or nothing. */
export async function fetchDraft(chatId: string, signal?: AbortSignal): Promise<ChatDraft | null> {
  const draft = await ok(api.GET('/chats/{id}/draft', { params: { path: { id: chatId } }, ...(signal === undefined ? {} : { signal }) }));
  // A person without a draft is answered with `null`, or with nothing at all.
  return draft ?? null;
}

export function saveDraft(chatId: string, body: ChatDraftSetBody): Promise<ChatDraft> {
  return ok(api.PUT('/chats/{id}/draft', { params: { path: { id: chatId } }, body }));
}

export function discardDraft(chatId: string): Promise<void> {
  return done(api.DELETE('/chats/{id}/draft', { params: { path: { id: chatId } } }));
}

// ---------------------------------------------------------------- chats among people

/**
 * Opens the direct chat with one person (the answer is the chat that exists already when it does), or
 * makes a group of several, with a name if one is given. `key` is made once, when the person acts.
 */
export function createChat(orgId: string, body: OrgChatCreateBody, key: string): Promise<Chat> {
  return ok(api.POST('/orgs/{id}/chats', { params: { path: { id: orgId } }, body, ...repeatable(key) }));
}

/** Gives a group a name, or names it after its members again with `null`. */
export function renameChat(chatId: string, name: string | null): Promise<Chat> {
  return ok(api.PATCH('/chats/{id}', { params: { path: { id: chatId } }, body: { name } }));
}

export function addMember(chatId: string, userId: string): Promise<Chat> {
  return ok(api.PUT('/chats/{id}/members/{user_id}', { params: { path: { id: chatId, user_id: userId } } }));
}

/** Takes a person out of a group, or lets the caller leave it. */
export function removeMember(chatId: string, userId: string): Promise<void> {
  return done(api.DELETE('/chats/{id}/members/{user_id}', { params: { path: { id: chatId, user_id: userId } } }));
}

// ---------------------------------------------------------------- forwarding

/** Forwards messages or items to several destinations at once, all or nothing. */
export function forward(body: ForwardCreateBody): Promise<ForwardCreateReply> {
  return ok(api.POST('/forward', { body }));
}
