/** Where a click on a mention goes: the block, the comment's block, the message or the turn it stands in. */
import type { LinkOccurrence, LinkedThing } from '@/generated/api-types';

/** The address of a source as a whole. */
export function sourcePath(source: Pick<LinkedThing, 'kind' | 'id'>): string {
  return source.kind === 'document' ? `/doc/${source.id}` : source.kind === 'chat' ? `/chat/${source.id}` : `/thread/${source.id}`;
}

/** The address of the source at one of its mentions; the source itself where the part has no address of its own. */
export function mentionPath(source: Pick<LinkedThing, 'kind' | 'id'>, anchor: LinkOccurrence['anchor']): string {
  const base = sourcePath(source);
  switch (anchor.type) {
    case 'block': return `${base}?block=${encodeURIComponent(anchor.block_id)}`;
    case 'comment': return anchor.block_id === null ? base : `${base}?block=${encodeURIComponent(anchor.block_id)}`;
    case 'message': return `${base}?message=${encodeURIComponent(anchor.message_id)}`;
    case 'turn': return anchor.turn === null ? base : `${base}?turn=${anchor.turn}`;
  }
}

/** The words that say where in the source a mention stands, as the key of the catalogue and its number. */
export function whereOf(anchor: LinkOccurrence['anchor']): { key: 'links.where.block' | 'links.where.comment' | 'links.where.message' | 'links.where.turn' | 'links.where.turn_number'; number?: number } {
  switch (anchor.type) {
    case 'block': return { key: 'links.where.block' };
    case 'comment': return { key: 'links.where.comment' };
    case 'message': return { key: 'links.where.message' };
    case 'turn': return anchor.turn === null ? { key: 'links.where.turn' } : { key: 'links.where.turn_number', number: anchor.turn };
  }
}
