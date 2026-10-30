/** A visible history panel owns the existing cache without opening another thread subscription. */
import { useEffect } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { holdThreadRead, threadWriter, useThreadStore } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadCacheKey, type ThreadReadTarget, type ThreadReadScope } from '@/realtime/threadScope';

export function useThreadReadScope(threadId: ThreadReadTarget | null): ThreadReadScope | null {
  const ref = useThreadReadRef(threadId ?? '');
  const key = threadReadCacheKey(ref);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const read = useStore(useThreadStore, (state) => threadId === null ? null : state.byThread.get(key)?.read ?? null);
  useEffect(() => {
    if (threadId === null) return;
    const release = holdThreadRead(ref);
    threadWriter.capture(ref, reader);
    return release;
  }, [threadId, ref, reader, read?.identity]);
  return read;
}
