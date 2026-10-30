/** The chats a node's panel offers: the node's own and those of the places above it that talk about the same work.
 *
 * On a thread the panel moves between the thread's chat, its topic's and its project's; on a topic, between
 * the topic's and the project's; on a project and on an org's home it holds the one chat of that place.
 * Which chat the composer beside the list sends to is always the one chosen, never another.
 */
import type { NodeKind } from '@/generated/api-types';

/** A place on the way down to a node, with its chat: a row of the tree has all of it. */
export interface Place {
  id: string;
  kind: NodeKind;
  name: string;
  chat: { id: string; seq: number };
}

export interface Segment {
  kind: NodeKind;
  id: string;
  name: string;
  chatId: string;
  /** The newest message sequence the page has heard of, for the unread dot. */
  seq: number;
}

/** The segments for a node, given the places from the root org down to it (the node last). */
export function segmentsFor(path: readonly Place[]): Segment[] {
  const node = path.at(-1);
  if (node === undefined) return [];
  const wanted: NodeKind[] = node.kind === 'org' || node.kind === 'project' ? [node.kind] : node.kind === 'thread' ? ['thread', 'topic', 'project'] : ['topic', 'project'];
  const chain: Segment[] = [];
  for (const kind of wanted) {
    const found = [...path].reverse().find((one) => one.kind === kind);
    if (found !== undefined) chain.push({ kind, id: found.id, name: found.name, chatId: found.chat.id, seq: found.chat.seq });
  }
  return chain;
}
