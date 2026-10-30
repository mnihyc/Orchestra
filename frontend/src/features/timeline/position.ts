/** A retained reading location names one authorized item; it stores no conversation content. */
import { keepPresentation, readPresentation } from '@/features/workbench/presentation';
import type { ThreadReadRef } from '@/realtime/threadScope';
import type { VirtualListPosition } from '@/ui/VirtualList';

export interface TimelinePosition extends VirtualListPosition { item: string | null }
const keyOf = (read: ThreadReadRef, selection: string | null) => JSON.stringify(['timeline-position', read.threadId, selection]);
export function readTimelinePosition(read: ThreadReadRef, selection: string | null): TimelinePosition | null {
  const value = read.key.kind === 'result' ? readPresentation(read.key.owner, keyOf(read, selection)) : null;
  if (typeof value !== 'object' || value === null || !('version' in value) || value.version !== 1 || !('key' in value) || !('item' in value)
    || !(value.key === null || typeof value.key === 'string') || !(value.item === null || typeof value.item === 'string')
    || !('offset' in value) || typeof value.offset !== 'number' || !Number.isFinite(value.offset) || value.offset < 0
    || !('atEnd' in value) || typeof value.atEnd !== 'boolean') return null;
  return { key: value.key, item: value.item, offset: value.offset, atEnd: value.atEnd };
}
export function keepTimelinePosition(read: ThreadReadRef, selection: string | null, position: TimelinePosition): void {
  if (read.key.kind === 'result') keepPresentation(read.key.owner, keyOf(read, selection), { version: 1, ...position });
}
