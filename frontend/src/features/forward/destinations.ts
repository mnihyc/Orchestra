/** Where a forward can go: the chats of the places a person can write in, their group chats, the people of the org and the agents' threads, and the sentence that says what the choice means.
 *
 * A destination is one of four kinds, each with its own icon: the chat of a place in the tree, a group
 * the person is in, a person, meaning the direct chat with them in the org the messages belong to, or
 * the agent of a thread, whose input the forward becomes, as the sender's own. Which places a person may
 * actually write in and which threads they may drive is the server's to say, and it says it for all of
 * them at once: one refusal and nothing is sent. The list offers every place the person can see whose
 * chat is not read-only and every thread they can see, narrowed by what they typed, and at most ten may
 * be chosen.
 */
import type { Chat, ForwardDestination, NodeKind, ThreadState, ThreadDisplay, TreeNode, UserRef } from '@/generated/api-types';

/** The most places one forward goes to, as the contract says. */
export const MOST_DESTINATIONS = 10;

export type Destination =
  | { key: string; kind: 'node'; chatId: string; label: string; detail: string | null; nodeKind: NodeKind; display?: ThreadDisplay }
  | { key: string; kind: 'group'; chatId: string; label: string; detail: string | null }
  | { key: string; kind: 'person'; userId: string; label: string; detail: string | null }
  | { key: string; kind: 'thread'; threadId: string; label: string; detail: string | null; state?: ThreadState; display?: ThreadDisplay };

const matches = (text: string, query: string): boolean => text.toLowerCase().includes(query.trim().toLowerCase());

/** The chats of the places of a tree, the most recently active first, narrowed by name; archived and deleted places take no message. */
export function nodeDestinations(nodes: Iterable<TreeNode>, query: string, pathOf: (node: TreeNode) => string | null): Destination[] {
  return [...nodes]
    .filter((node) => !node.deleted && !node.archived && (query.trim() === '' || matches(node.name, query)))
    .sort((one, other) => other.chat.seq - one.chat.seq || one.name.localeCompare(other.name))
    .map((node) => ({ key: `chat:${node.chat.id}`, kind: 'node' as const, chatId: node.chat.id, label: node.name, detail: pathOf(node), nodeKind: node.kind, ...(node.thread?.display === undefined ? {} : { display: node.thread.display }) }));
}

/** The agents' threads of a tree, the most recently active first, narrowed by name: what is forwarded there is queued as an
 * input of the sender's. Archived and deleted threads take none, and the thread the items come from is not offered. */
export function threadDestinations(
  nodes: Iterable<TreeNode>,
  query: string,
  pathOf: (node: TreeNode) => string | null,
  except: string | null = null,
): Destination[] {
  return [...nodes]
    .filter((node) => node.kind === 'thread' && !node.deleted && !node.archived && node.id !== except && (query.trim() === '' || matches(node.name, query)))
    .sort((one, other) => (other.thread?.activity_seq ?? 0) - (one.thread?.activity_seq ?? 0) || one.name.localeCompare(other.name))
    .map((node) => ({ key: `thread:${node.id}`, kind: 'thread' as const, threadId: node.id, label: node.name, detail: pathOf(node), state: node.thread?.state ?? 'idle', ...(node.thread?.display === undefined ? {} : { display: node.thread.display }) }));
}

/** The groups the person is in, narrowed by name or by the names of their people. */
export function groupDestinations(chats: readonly Chat[], query: string, me: string | null): Destination[] {
  return chats
    .filter((chat) => chat.kind === 'group' && chat.can.send)
    .map((chat) => ({ chat, label: groupLabel(chat, me) }))
    .filter(({ label }) => query.trim() === '' || matches(label, query))
    .map(({ chat, label }) => ({ key: `chat:${chat.id}`, kind: 'group' as const, chatId: chat.id, label, detail: null }));
}

/** A group's name, or its people's names where it has none. */
export function groupLabel(chat: Pick<Chat, 'name' | 'members'>, me: string | null): string {
  if (chat.name !== null) return chat.name;
  return (chat.members ?? []).filter((member) => member.id !== me).map((member) => member.display_name).join(', ');
}

/** The people found, but never the person themselves. */
export function personDestinations(people: readonly UserRef[], me: string | null): Destination[] {
  return people
    .filter((user) => user.id !== me && user.kind === 'human')
    .map((user) => ({ key: `user:${user.id}`, kind: 'person' as const, userId: user.id, label: user.display_name, detail: `@${user.username}` }));
}

/** The request's destinations for the ones chosen. */
export function destinationsOf(chosen: readonly Destination[]): ForwardDestination[] {
  return chosen.map((one) => (one.kind === 'person' ? { user_id: one.userId } : one.kind === 'thread' ? { thread_id: one.threadId } : { chat_id: one.chatId }));
}

/** How the choice is told: how many chats, which people, or how many people when there are several, and which agents' threads. */
export function summary(chosen: readonly Destination[]): { chats: number; people: string[]; threads: string[]; places: number } {
  return {
    chats: chosen.filter((one) => one.kind === 'node' || one.kind === 'group').length,
    people: chosen.filter((one) => one.kind === 'person').map((one) => one.label),
    threads: chosen.filter((one) => one.kind === 'thread').map((one) => one.label),
    places: chosen.length,
  };
}

/** Which of the chosen destinations a refusal names in its `object`, if it names one. */
export function refusedDestination(chosen: readonly Destination[], object: unknown): Destination | null {
  if (typeof object !== 'object' || object === null) return null;
  const named = object as { chat_id?: unknown; user_id?: unknown; thread_id?: unknown };
  return (
    chosen.find((one) =>
      one.kind === 'person' ? one.userId === named.user_id : one.kind === 'thread' ? one.threadId === named.thread_id : one.chatId === named.chat_id,
    ) ?? null
  );
}
