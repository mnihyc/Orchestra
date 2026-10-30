/** What a person does in a chat: sending, with the message shown at once and sent again while the line is bad, editing, deleting, reacting, pinning and reading.
 *
 * A message is shown where it will stand the moment it is sent, as "Sending…", under the person's
 * name, and becomes the server's row when the server answers; if the line is lost it is tried again
 * three times, after one, two and four seconds, with the key it was made with, so that a second
 * attempt after a lost answer makes nothing twice; if it still does not go, it stays in the chat with
 * its text and "Not sent · Try again". A reaction and a read mark are the person's own and easily
 * undone: they show at once and are put right if the server refuses. Everything that changes what
 * others see waits for the answer and is marked as under way meanwhile.
 */
import { create } from 'zustand';

import { ApiError } from '@/api/client';
import { deleteMessage, editMessage, markRead, pin as pinMessage, react as reactTo, sendMessage } from '@/api/chats';
import type { ChatMentionName, ChatMessage, ChatPart, UserRef } from '@/generated/api-types';
import { NetworkError, newActKey, retryPause, READ_RETRY_PAUSES_MS } from '@/lib/requests';
import { chatWriter, refreshChatStates, statesWriter, useChatStates, useChatStore } from '@/realtime/stores/chat';
import { i18n } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { errorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

// ---------------------------------------------------------------- what is under way

interface Busy {
  /** The messages whose change has been asked for and not answered. */
  ids: ReadonlySet<string>;
}

export const useBusy = create<Busy>(() => ({ ids: new Set() }));

function busy(id: string, on: boolean): void {
  useBusy.setState((state) => {
    const ids = new Set(state.ids);
    if (on) ids.add(id);
    else ids.delete(id);
    return { ids };
  });
}

/** Runs an act that changes what others see: the message is marked as under way until it is answered. */
async function waiting<Result>(messageId: string, act: () => Promise<Result>): Promise<Result> {
  busy(messageId, true);
  try {
    return await act();
  } finally {
    busy(messageId, false);
  }
}

/** Says why an act failed, as a toast: in the reader's words, what the server refused with or that the line failed. */
export function failed(title: string, error: unknown): void {
  const { locale, timeZone } = useAppearance.getState();
  toast({ tone: 'danger', title, description: errorText(error, i18n.t.bind(i18n), { locale, timeZone }).message });
}

// ---------------------------------------------------------------- sending

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function attempt(chatId: string, clientId: string): Promise<void> {
  for (let tries = 0; ; tries += 1) {
    const sent = useChatStore.getState().byChat.get(chatId)?.outbox.find((one) => one.clientId === clientId);
    if (sent === undefined) return;
    try {
      const reply = await sendMessage(chatId, {
        client_id: clientId,
        parts: sent.parts,
        ...(sent.replyTo === null ? {} : { reply_to: sent.replyTo }),
        ...(sent.agentReview === undefined ? {} : { agent_review: sent.agentReview }),
      });
      chatWriter.own(chatId, reply.message);
      chatWriter.outgoingDone(chatId, clientId);
      // What a person wrote they have read, and writing in a chat makes them follow it: both are read again from the server.
      read(chatId, reply.message.seq_created);
      refreshChatStates();
      return;
    } catch (error) {
      // A line that was lost is tried again with the same key; an answer of the server is final.
      if (error instanceof NetworkError && tries < READ_RETRY_PAUSES_MS.length) {
        await sleep(retryPause(tries));
        continue;
      }
      chatWriter.outgoingChanged(chatId, clientId, { state: 'failed', error: error instanceof ApiError ? error : null });
      return;
    }
  }
}

/** Sends a message: it stands in the chat at once as one that is being sent. Returns the key it was made with. */
export function send(chatId: string, parts: ChatPart[], replyTo: string | null, mentionNames: ChatMentionName[] = [], agentReview?: string): string {
  const clientId = newActKey();
  chatWriter.outgoing(chatId, { clientId, parts, replyTo, mentionNames, ...(agentReview === undefined ? {} : { agentReview }), at: Date.now(), state: 'sending', error: null });
  void attempt(chatId, clientId);
  return clientId;
}

/** Tries a message again that was not sent. */
export function resend(chatId: string, clientId: string): void {
  chatWriter.outgoingChanged(chatId, clientId, { state: 'sending', error: null });
  void attempt(chatId, clientId);
}

/** Gives up a message that was not sent. */
export function discard(chatId: string, clientId: string): void {
  chatWriter.outgoingDone(chatId, clientId);
}

// ---------------------------------------------------------------- changing a message

export async function edit(chatId: string, messageId: string, parts: ChatPart[]): Promise<boolean> {
  try {
    const changed = await waiting(messageId, () => editMessage(chatId, messageId, parts));
    chatWriter.own(chatId, changed);
    return true;
  } catch (error) {
    failed(i18n.t('chat.failed.edit'), error);
    return false;
  }
}

export async function remove(chatId: string, messageId: string): Promise<boolean> {
  try {
    const changed = await waiting(messageId, () => deleteMessage(chatId, messageId));
    chatWriter.own(chatId, changed);
    return true;
  } catch (error) {
    failed(i18n.t('chat.failed.delete'), error);
    return false;
  }
}

export async function pin(chatId: string, messageId: string, on: boolean): Promise<void> {
  try {
    const changed = await waiting(messageId, () => pinMessage(chatId, messageId, on));
    chatWriter.own(chatId, changed);
  } catch (error) {
    failed(i18n.t(on ? 'chat.failed.pin' : 'chat.failed.unpin'), error);
  }
}

/** Whether a person has reacted to a message with an emoji. */
export function reacted(message: ChatMessage, emoji: string, me: string | null): boolean {
  return me !== null && (message.reactions.find((one) => one.emoji === emoji)?.users.includes(me) ?? false);
}

/** The message as it reads with the person's reaction added or taken away. */
export function withReaction(message: ChatMessage, emoji: string, me: string, on: boolean): ChatMessage {
  const held = message.reactions.find((one) => one.emoji === emoji);
  if (on) {
    if (held?.users.includes(me) === true) return message;
    const reactions =
      held === undefined
        ? [...message.reactions, { emoji, users: [me] }]
        : message.reactions.map((one) => (one.emoji === emoji ? { ...one, users: [...one.users, me] } : one));
    return { ...message, reactions };
  }
  if (held?.users.includes(me) !== true) return message;
  const reactions = message.reactions
    .map((one) => (one.emoji === emoji ? { ...one, users: one.users.filter((user) => user !== me) } : one))
    .filter((one) => one.users.length > 0);
  return { ...message, reactions };
}

/** Toggles the person's own reaction: shown at once, and put back if the server refuses. */
export async function toggleReaction(chatId: string, message: ChatMessage, emoji: string, me: string): Promise<void> {
  const on = !reacted(message, emoji, me);
  const shown = withReaction(message, emoji, me, on);
  chatWriter.shown(chatId, shown);
  try {
    chatWriter.own(chatId, await reactTo(chatId, message.id, emoji, on));
  } catch (error) {
    chatWriter.unshown(chatId, shown, message);
    failed(i18n.t('chat.failed.react'), error);
  }
}

// ---------------------------------------------------------------- reading

/** Says that the person has read a chat up to a sequence: shown at once. */
export function read(chatId: string, seq: number): void {
  const held = useChatStates.getState().states.get(chatId) ?? useChatStore.getState().byChat.get(chatId)?.chat?.my ?? null;
  if (held !== null && held.last_read_seq >= seq) return;
  if (held !== null) statesWriter.read(chatId, seq, held);
  // If the server did not take it, the next read says it again, and the states are read anew whenever the line comes back.
  markRead(chatId, seq, held?.read_revision ?? 0).catch(() => undefined);
}

/** The names of the people who reacted, from the people the page knows by their id; those it does not know are counted. */
export function namesOf(ids: readonly string[], known: ReadonlyMap<string, Pick<UserRef, 'display_name'>>): { names: string[]; others: number } {
  const names: string[] = [];
  let others = 0;
  for (const id of ids) {
    const user = known.get(id);
    if (user === undefined) others += 1;
    else names.push(user.display_name);
  }
  return { names, others };
}
