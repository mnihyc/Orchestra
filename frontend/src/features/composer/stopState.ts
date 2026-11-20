/** Every control of this reader's thread shares the in-flight request before its durable observation arrives. */
import { useIsMutating } from '@tanstack/react-query';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadCacheKey } from '@/realtime/threadScope';

export function useStopRequest(threadId: string) {
  const read = useThreadReadRef(threadId);
  const reader = useThreadPart(threadId, (view) => view.read?.reader ?? null);
  const key = ['thread-stop', reader, threadReadCacheKey(read)] as const;
  const observed = useThreadPart(threadId, (view) => view.stopping);
  const completed = observed?.pending === false ? observed.version : 0;
  // A response still in flight belongs to the Stop that was current when the person pressed it.
  const pending = useIsMutating({ mutationKey: key, predicate: (mutation) => {
    const before = mutation.state.context;
    return typeof before !== 'number' || before >= completed;
  } }) > 0;
  return { key, version: observed?.version ?? 0, pending };
}
