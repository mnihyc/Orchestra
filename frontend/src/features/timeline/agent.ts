/** Which agent said or did a thing: the agent of the segment the item belongs to, else the one the thread is bound to now.
 *
 * A conversation can pass through several agents (a switch starts a new segment), and each item is
 * read as the agent of its own time said it.
 */
import type { AgentRef } from '@/generated/api-types';
import { useThreadPart } from '@/realtime/stores/thread';

export function useAgentOf(threadId: string, segmentId: string | null): AgentRef | null {
  return useThreadPart(threadId, (view) => {
    const segment = segmentId === null ? undefined : view.segments.find((candidate) => candidate.id === segmentId);
    return segment?.agent ?? view.thread?.binding?.agent ?? null;
  });
}
