/** What a hit of a search is to a reader: where it lies, who wrote it and when, the words around it with the match marked, and where it opens.
 *
 * A hit is an item of a conversation, a message of a chat, or a document (its title, the name of a file it
 * holds, or one block). The server counts the ranges of the match in characters, which are code points,
 * and a string in the page counts UTF-16 units: a snippet with an emoji in it would be marked a letter
 * off, so the ranges are turned into units here before anything is drawn.
 */
import type { Destination } from '@/features/finder/destinations';
import { labelsAlongPath } from '@/features/search/labels';
import type { Author, NodeRef, SearchHit } from '@/generated/api-types';

/** A part of a snippet and whether it is a match. */
export interface Excerpt {
  text: string;
  marked: boolean;
}

/** Cuts a snippet along its matches. Ranges outside the snippet are ignored and overlapping ones are one. */
export function excerptOf(snippet: string, matches: readonly (readonly [number, number])[]): Excerpt[] {
  const characters = Array.from(snippet);
  const ranges = matches
    .map(([from, to]) => [Math.max(0, from), Math.min(characters.length, to)] as const)
    .filter(([from, to]) => to > from)
    .sort((one, other) => one[0] - other[0]);
  const parts: Excerpt[] = [];
  let at = 0;
  for (const [from, to] of ranges) {
    if (to <= at) continue;
    const start = Math.max(from, at);
    if (start > at) parts.push({ text: characters.slice(at, start).join(''), marked: false });
    const text = characters.slice(start, to).join('');
    const last = parts.at(-1);
    // Matches that touch are one mark.
    if (last?.marked === true) last.text += text;
    else parts.push({ text, marked: true });
    at = to;
  }
  if (at < characters.length) parts.push({ text: characters.slice(at).join(''), marked: false });
  return parts.length === 0 ? [{ text: snippet, marked: false }] : parts;
}

/** The places a hit lies in, from the root org down: the thread's own way, the chat's node, the document's node. */
export function wayOfHit(hit: SearchHit): NodeRef[] {
  switch (hit.kind) {
    case 'name': return hit.path;
    case 'item':
      // The server names the thread, its topic and its project; the orgs above them are not in the hit.
      return [hit.project, hit.topic, hit.thread];
    case 'chat_message':
    case 'document':
      return hit.path;
  }
}

/**
 * Where a hit lies as the names of its places, from the root down, ending with the thing itself: the thread, the
 * document, the chat. A place the reader may not know the name of is told as `private(kind)`.
 */
export function crumbsOf(hit: SearchHit, words: { chat: string; private: (kind: NodeRef['kind']) => string }, labels: ReadonlyMap<string, string> = new Map()): string[] {
  const path = labelsAlongPath(wayOfHit(hit), labels, words.private);
  switch (hit.kind) {
    case 'name': return path;
    case 'item':
      return path;
    case 'chat_message':
      return [...path, ...(hit.chat.node === null ? [hit.chat.name ?? words.chat] : [])];
    case 'document':
      return [...path, hit.document.title];
  }
}

/** Who wrote it, as far as the hit says. */
export function authorOfHit(hit: SearchHit): Author | null {
  switch (hit.kind) {
    case 'name': return null;
    case 'item':
      return hit.item.author;
    case 'chat_message':
      return hit.message.author;
    case 'document':
      return hit.by ?? (hit.block_id === null ? hit.document.updated_by ?? hit.document.created_by : null);
  }
}

/** When it was written. */
export function atOfHit(hit: SearchHit): string {
  switch (hit.kind) {
    case 'name': return hit.at;
    case 'item':
      return hit.item.created_at;
    case 'chat_message':
      return hit.message.created_at;
    case 'document':
      return hit.document.updated_at;
  }
}

/** A key that no other hit has: the same document can be found in several blocks. */
export function keyOfHit(hit: SearchHit): string {
  switch (hit.kind) {
    case 'name': return `name:${hit.node.id}`;
    case 'item':
      return `item:${hit.item.id}`;
    case 'chat_message':
      return `chat:${hit.message.id}`;
    case 'document':
      return `doc:${hit.document.id}:${hit.block_id ?? 'title'}`;
  }
}

/** Where a hit opens: the thread at the item, the chat at the message, the document at the block. */
export function destinationOfHit(hit: SearchHit, slugOfOrg: (orgId: string) => string | null): Destination | null {
  switch (hit.kind) {
    case 'name': return { to: hit.href };
    case 'item':
      return { to: '/thread/$threadId', params: { threadId: hit.thread.id }, search: { item: hit.item.id,
        ...(hit.projection === undefined || hit.projection === null ? {} : { continuation: hit.projection.viewed_continuation_id }) } };
    case 'document':
      return { to: '/doc/$documentId', params: { documentId: hit.document.id }, search: { block: hit.block_id ?? undefined } };
    case 'chat_message': {
      const node = hit.chat.node;
      if (node !== null) {
        // A place's chat is read beside the place.
        const search = { panel: 'chat', chat: hit.chat.id, message: hit.message.id };
        switch (node.kind) {
          case 'org':
            return { to: '/org/$slug', params: { slug: node.id }, search };
          case 'project':
            return { to: '/project/$projectId', params: { projectId: node.id }, search };
          case 'topic':
            return { to: '/topic/$topicId', params: { topicId: node.id }, search };
          case 'thread':
            return { to: '/thread/$threadId', params: { threadId: node.id }, search };
        }
      }
      const slug = slugOfOrg(hit.chat.org_id);
      return slug === null ? null : { to: '/org/$slug/messages', params: { slug }, search: { chat: hit.chat.id, message: hit.message.id } };
    }
  }
}
