/** The rows of a conversation: its messages with day dividers, the line where the new ones begin, and which messages share the header of the one before.
 *
 * What is drawn is a list of rows, and this is where the list is decided, so that it can be held
 * to its rules by tests that draw nothing: consecutive messages of one author within five minutes
 * share one header; a day divider stands where the day changes; the "New" line stands above the first
 * message that the reader has not seen and did not write; a group's events are lines of their own and
 * break a run of messages.
 */
import type { ChatMessage } from '@/generated/api-types';

/** Messages of one author this close to each other read as one thing said in several lines. */
export const GROUP_WITHIN_MS = 5 * 60_000;

export type ConversationRow =
  | { kind: 'day'; key: string; at: string }
  | { kind: 'new'; key: string }
  | { kind: 'system'; key: string; message: ChatMessage }
  | { kind: 'message'; key: string; message: ChatMessage; head: boolean };

export interface RowsOptions {
  /** How far the reader had read when the chat was opened. Messages after it that are not theirs are new; `null` marks nothing. */
  readTo: number | null;
  /** The reader, so that what they wrote themselves is never new. */
  me: string | null;
  /** The calendar day a moment falls on, in the reader's time zone. */
  dayOf: (moment: string) => string;
}

/** Who wrote a message, as one string: a person, an agent in a thread, or nobody. */
export function authorKey(message: ChatMessage): string {
  const author = message.author;
  if (author === null) return 'system';
  if ('agent' in author) return `agent:${author.thread.id}:${author.agent}`;
  return `user:${author.id}`;
}

/** Whether a message was written by this person. */
export function isMine(message: ChatMessage, me: string | null): boolean {
  const author = message.author;
  return me !== null && author !== null && 'id' in author && author.id === me;
}

export function conversationRows(messages: readonly ChatMessage[], options: RowsOptions): ConversationRow[] {
  const rows: ConversationRow[] = [];
  let day: string | null = null;
  let previous: ChatMessage | null = null;
  let marked = false;
  for (const message of messages) {
    const thatDay = options.dayOf(message.created_at);
    if (thatDay !== day) {
      rows.push({ kind: 'day', key: `day:${thatDay}`, at: message.created_at });
      day = thatDay;
      previous = null;
    }
    const isNew =
      !marked && options.readTo !== null && message.kind === 'message' && message.seq_created > options.readTo && !isMine(message, options.me);
    if (isNew) {
      rows.push({ kind: 'new', key: 'new' });
      marked = true;
      previous = null;
    }
    if (message.kind === 'system') {
      rows.push({ kind: 'system', key: message.id, message });
      previous = null;
      continue;
    }
    const joins =
      previous !== null &&
      authorKey(previous) === authorKey(message) &&
      Date.parse(message.created_at) - Date.parse(previous.created_at) < GROUP_WITHIN_MS;
    rows.push({ kind: 'message', key: message.id, message, head: !joins });
    previous = message;
  }
  return rows;
}

/** How many messages are new to the reader: those after `readTo` that they did not write. */
export function newCount(messages: readonly ChatMessage[], readTo: number | null, me: string | null): number {
  if (readTo === null) return 0;
  return messages.filter((message) => message.kind === 'message' && message.seq_created > readTo && !isMine(message, me)).length;
}
