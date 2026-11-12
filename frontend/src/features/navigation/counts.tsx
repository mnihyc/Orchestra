/** How many of the person's chats in an org are unread: the number on Messages in the left bar and the dot on the org in the rail.
 *
 * The numbers of the Inbox and of the approvals are not here: the person's own feed keeps them (`features/inbox/Counters`), and the rail
 * and the bar of a small screen both draw them from there.
 *
 * Chats are counted from the person's bootstrap and live states, including their root org and newest
 * message. No chat-list read is needed: a chat is unread when they
 * follow it and its newest message is beyond what they read. A chat they hid is not counted.
 */
import { useMemo } from 'react';

import type { ChatState } from '@/generated/api-types';
import { isUnread, useChatStates } from '@/realtime/stores/chat';

// ---------------------------------------------------------------- the chats of an org

/** The newest message heard of in a chat, by chat. */
type Latest = ReadonlyMap<string, { seq: number }>;

/**
 * Followed, visible states in this root org whose newest message is beyond the person's read position.
 * Personal activity can advance `latest` before a state frame arrives.
 */
export function unreadChats(orgId: string | null, states: ReadonlyMap<string, ChatState>, latest: Latest): number {
  if (orgId === null) return 0;
  let count = 0;
  for (const state of states.values()) {
    if (state.org_id !== orgId || state.hidden) continue;
    if (isUnread(state, Math.max(state.last_message_seq, latest.get(state.chat_id)?.seq ?? 0))) count += 1;
  }
  return count;
}

/** How many of the person's own chats in a root org are unread, kept current as messages arrive and are read. */
export function useUnreadChats(orgId: string | null): number {
  const states = useChatStates((state) => state.states);
  const latest = useChatStates((state) => state.latest);
  return useMemo(() => unreadChats(orgId, states, latest), [orgId, states, latest]);
}
