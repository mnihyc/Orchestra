/** Ended sessions keep their reported work, but cannot acquire a live clock or a running header chip. */
import type { Item, Segment } from '@/generated/api-types';
import { threadReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { useTimelineContext } from '@/features/timeline/context';

export function recordedItem(item: Pick<Item, 'before_orch' | 'segment_id'>, segments: readonly Segment[]): boolean {
  return item.before_orch || segments.some((segment) => segment.id === item.segment_id && segment.ended_at !== null);
}

export function useRecordedItem(threadId: string, item: Pick<Item, 'before_orch' | 'segment_id' | 'continuation_id'>): boolean {
  const { inspection } = useTimelineContext();
  return useThreadPart(threadId, (view) => inspection !== undefined || recordedItem(item, view.segments) || view.read?.projection !== undefined
    && view.read.projection !== null && (threadReadOnly(view) || item.continuation_id !== view.read.projection.viewed_continuation_id));
}
