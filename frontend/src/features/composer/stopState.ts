/** Every control of this reader's thread shares the in-flight request before its durable observation arrives. */
import { useIsMutating } from '@tanstack/react-query';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadCacheKey } from '@/realtime/threadScope';

export function useStopRequest(threadId: string) {
  const read = useThreadReadRef(threadId);
  const reader = useThreadPart(threadId, (view) => view.read?.reader ?? null);
  const key = ['thread-stop', reader, threadReadCacheKey(read)] as const;
  return { key, pending: useIsMutating({ mutationKey: key }) > 0 };
}
