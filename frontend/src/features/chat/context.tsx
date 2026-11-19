/** What the parts of an open chat share: the chat, who is reading it, the people known in it, and what can be done to a message.
 *
 * A message, the list it stands in and the composer under it all need the same few things, and each
 * of them acts on the same chat. They are given once, by the chat view, and read here, so that no
 * part has to be handed a dozen props to pass on.
 */
import { createContext, useContext, useMemo } from 'react';

import type { Chat, ChatMessage, UserRef } from '@/generated/api-types';
import type { ChatEntry } from '@/realtime/stores/chat';

/** What can be done to a message from its toolbar and its menu. */
export interface MessageActions {
  unread?: (message: ChatMessage) => void;
  react: (message: ChatMessage, emoji: string) => void;
  reply: (message: ChatMessage) => void;
  edit: (message: ChatMessage) => void;
  /** Asks first, naming the message, and deletes. */
  remove: (message: ChatMessage) => void;
  pin: (message: ChatMessage, on: boolean) => void;
  forward: (message: ChatMessage) => void;
  select: (message: ChatMessage) => void;
  copyText: (message: ChatMessage) => void;
  copyLink: (message: ChatMessage) => void;
  /** Scrolls to a message, loading the part of the chat it is in if it is not held. */
  jump: (messageId: string) => void;
}

export interface ChatContextValue {
  chat: Chat;
  /** The reader. */
  me: string | null;
  /** Everybody the page knows in this chat, by id. */
  people: ReadonlyMap<string, UserRef>;
  acts: MessageActions;
  /** The message being edited in place, if any. */
  editing: string | null;
  setEditing: (messageId: string | null) => void;
  /** Whether messages are being picked for forwarding, and which. */
  selection: { active: boolean; has: (messageId: string) => boolean; toggle: (messageId: string, range: boolean) => void };
  /** The message that was jumped to and is lit for a moment. */
  lit: string | null;
}

export const ChatContext = createContext<ChatContextValue | null>(null);

export function useChatContext(): ChatContextValue {
  const value = useContext(ChatContext);
  if (value === null) throw new Error('a part of a chat was drawn outside the chat');
  return value;
}

/** The people a chat has shown the page, by id: its members, the authors of its messages, whoever was named, replied to or is looking. */
export function usePeople(entry: ChatEntry): ReadonlyMap<string, UserRef> {
  const { chat, messages, presence, typing } = entry;
  return useMemo(() => {
    const known = new Map<string, UserRef>();
    const add = (user: UserRef | null | undefined) => {
      if (user !== null && user !== undefined) known.set(user.id, user);
    };
    for (const member of chat?.members ?? []) add(member);
    add(chat?.created_by);
    for (const message of messages.values()) {
      if (message.author !== null && 'id' in message.author) add(message.author);
      for (const mentioned of message.mentions) add(mentioned);
      const replied = message.reply_to?.author;
      if (replied !== null && replied !== undefined && 'id' in replied) add(replied);
      add(message.pinned?.by);
      add(message.system?.by);
      add(message.system?.user);
    }
    for (const looking of presence) add(looking.user);
    for (const mark of typing.values()) add(mark.user);
    return known;
  }, [chat, messages, presence, typing]);
}
