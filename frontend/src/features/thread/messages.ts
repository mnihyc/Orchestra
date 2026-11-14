/** The people's messages of a thread, in the order they were said: what the keys, the Messages list and the sticky prompt move between.
 *
 * A person's message is what a person said to the agent: not an announcement, a goal's input or a line of the system.
 * They are read from the rows the page holds, so that the position of the reader among them is known without a request.
 */
import { itemText } from '@/features/timeline/ItemActions';
import type { Item, UserRef } from '@/generated/api-types';

type UserItem = Extract<Item, { kind: 'user_message' }>;

export interface PersonMessage {
  id: string;
  seq: number;
  at: string;
  author: UserRef;
  /** What was said, whole: the Messages list is searched by it. */
  text: string;
  /** The first line of what was said. */
  line: string;
  turnId: string | null;
  /** The lines the turn that began with this message added and removed, where it changed files. */
  added: number;
  removed: number;
}

export function isPersonMessage(item: Item): item is UserItem & { author: UserRef } {
  return item.kind === 'user_message' && item.payload.source === 'user' && item.author !== null && item.stream === 'main';
}

/** What every turn's file changes added and removed, in one pass over the conversation. */
export function lineCountsByTurn(items: ReadonlyMap<string, Item>, order: readonly string[]): ReadonlyMap<string, { added: number; removed: number }> {
  const counts = new Map<string, { added: number; removed: number }>();
  for (const id of order) {
    const item = items.get(id);
    if (item?.kind !== 'tool_call' || item.turn_id === null || item.payload.tool.kind !== 'file_change') continue;
    const held = counts.get(item.turn_id) ?? { added: 0, removed: 0 };
    for (const change of item.payload.tool.changes) {
      held.added += change.added;
      held.removed += change.removed;
    }
    counts.set(item.turn_id, held);
  }
  return counts;
}

/** The people's messages the page holds, oldest first. */
export function personMessages(items: ReadonlyMap<string, Item>, order: readonly string[]): PersonMessage[] {
  const counts = lineCountsByTurn(items, order);
  return order.flatMap((id) => {
    const item = items.get(id);
    if (item === undefined || !isPersonMessage(item)) return [];
    const changed = item.turn_id === null ? undefined : counts.get(item.turn_id);
    const text = itemText(item);
    return [{ id: item.id, seq: item.seq_created, at: item.created_at, author: item.author, text, line: text.trim().split(/\r?\n/)[0] ?? '',
      turnId: item.turn_id, added: changed?.added ?? 0, removed: changed?.removed ?? 0 }];
  });
}

/** The message the reader at `seq` is in the middle of: the last one said at or before that place. */
export function messageAt(messages: readonly PersonMessage[], seq: number): PersonMessage | null {
  return messages.findLast((message) => message.seq <= seq) ?? null;
}

/** The message before (`-1`) or after (`1`) a place in the conversation, not counting one that stands at it. */
export function messageBeside(messages: readonly PersonMessage[], seq: number, direction: -1 | 1): PersonMessage | null {
  return (direction === 1 ? messages.find((message) => message.seq > seq) : messages.findLast((message) => message.seq < seq)) ?? null;
}
