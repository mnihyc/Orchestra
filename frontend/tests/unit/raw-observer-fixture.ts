/** Existing request/owner tests join raw through its real observer and ACK gate, never an HTTP-only bypass. */
import { InfiniteQueryObserver } from '@tanstack/react-query';
import { expect, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { captureThreadRead } from '@/api/threadRead';
import { viewedRawFramesQuery } from '@/api/threadHistory';
import { installRawQueryFeeds } from '@/realtime/rawQuery';
import { createRealtimeSocket } from '@/realtime/socket';
import type { ThreadReadTarget } from '@/realtime/threadScope';
import { Wire } from './continuation-fixtures';

export async function readAdmittedRaw(target: ThreadReadTarget, segment: string) {
  const captured = captureThreadRead(target), projection = captured.scope.projection;
  if (projection === null) throw new Error('The actual owning history must already be open');
  const wire = new Wire();
  const socket = createRealtimeSocket({ url: () => 'ws://orch.test/api/v1/ws', token: () => 'synthetic-observer-reader',
    renew: () => Promise.resolve(true), onSuspended: () => undefined, random: () => 0.5, connect: () => wire });
  const stop = installRawQueryFeeds(socket, queryClient);
  const options = viewedRawFramesQuery(target, segment), observer = new InfiniteQueryObserver(queryClient, options);
  const off = observer.subscribe(() => undefined);
  try {
    socket.start(); wire.ready();
    const sub = await wire.subscriptions.next();
    expect(sub.feed).toBe(`thread:${captured.threadId}:raw`);
    expect(sub.raw_segment_id).toBe(segment);
    expect(sub.projection_revision).toBe(projection.projection_revision);
    expect(sub.continuation_id ?? null).toBe(captured.scope.selection);
    expect(observer.getCurrentResult().data).toBeUndefined();
    wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'ephemeral', seq: null, projection,
      raw: { segment_id: segment, after: 0 } });
    await vi.advanceTimersByTimeAsync(0);
    const result = observer.getCurrentResult();
    if (result.error !== null) throw result.error;
    expect(result.isSuccess).toBe(true);
    expect(result.data?.pages).toHaveLength(1);
    return options;
  } finally { off(); stop(); socket.stop(); }
}
