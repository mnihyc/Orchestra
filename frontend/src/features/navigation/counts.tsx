/** How many of the person's chats in an org are unread: the number on Messages in the left bar and the dot on the org in the rail.
 *
 * The numbers of the Inbox and of the approvals are not here: the person's own feed keeps them (`features/inbox/Counters`), and the rail
 * and the bar of a small screen both draw them from there.
 *
 * The chats of an org are counted from the list the Messages screen itself shows and from the
 * person's standing in each chat, which their own feed keeps current: a chat is unread when they
 * follow it and its newest message is beyond what they read. A chat they hid is not counted.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useMemo } from 'react';

import { orgChatsQuery } from '@/api/chats';
import type { Chat, ChatState } from '@/generated/api-types';
import { isUnread, useChatStates } from '@/realtime/stores/chat';

// ---------------------------------------------------------------- the chats of an org

/** The newest message heard of in a chat, by chat. */
type Latest = ReadonlyMap<string, { seq: number }>;

/**
 * How many of a person's direct and group chats are unread: those they follow whose newest message is
 * beyond what they read, without the ones they hid. `states` is their standing as their feed keeps
 * it, which is newer than what a chat said when the list was read; `latest` the newest message heard of.
 */
export function unreadChats(chats: readonly Pick<Chat, 'id' | 'last_message_seq' | 'my'>[], states: ReadonlyMap<string, ChatState>, latest: Latest): number {
  let count = 0;
  for (const chat of chats) {
    const state = states.get(chat.id) ?? chat.my;
    if (state === null || state.hidden) continue;
    if (isUnread(state, Math.max(chat.last_message_seq, latest.get(chat.id)?.seq ?? 0))) count += 1;
  }
  return count;
}

/** How many of the person's own chats in a root org are unread, kept current as messages arrive and are read. */
export function useUnreadChats(orgId: string | null, load = false): number {
  // Avatars read known counts; opening this org's Navigation is what reads its own chats.
  const mine = useInfiniteQuery({ ...orgChatsQuery(orgId ?? '', { scope: 'mine', of: null }), enabled: load && orgId !== null });
  const states = useChatStates((state) => state.states);
  const latest = useChatStates((state) => state.latest);
  return useMemo(() => unreadChats(mine.data?.pages.flatMap((page) => page.items) ?? [], states, latest), [mine.data, states, latest]);
}
