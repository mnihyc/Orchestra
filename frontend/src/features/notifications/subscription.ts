/** Applies the ordinary subscription reply to its existing page, bell and navigation projections. */
import type { QueryClient } from '@tanstack/react-query';
import { notificationExceptionsKey, notificationExceptionsQuery } from '@/api/notificationExceptions';
import { nodeKey } from '@/api/nodes';
import { attentionWriter } from '@/features/navigation/attention';
import type { Node, NotificationExceptionChanged } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';

/** There is one shown exceptions page; a bell includes it in the same native write's answer. */
export function exceptionCursor(cache: QueryClient): string | null {
  const query = cache.getQueryCache().findAll({ queryKey: notificationExceptionsKey }).find((entry) => entry.isActive());
  const cursor = query?.queryKey[1];
  return typeof cursor === 'string' ? cursor : null;
}

/** No follow-up read: the server has already recomputed the affected page and actual access. */
export async function acceptSubscription(cache: QueryClient, id: string, reply: NotificationExceptionChanged, cursor: string | null): Promise<void> {
  // A read begun before this write cannot replace its newer authoritative acknowledgement.
  await Promise.all([
    cache.cancelQueries({ queryKey: notificationExceptionsKey }),
    cache.cancelQueries({ queryKey: nodeKey(id), exact: true }),
  ]);
  void cache.invalidateQueries({ queryKey: notificationExceptionsKey, refetchType: 'none' });
  cache.setQueryData(notificationExceptionsQuery(reply.page_cursor ?? cursor).queryKey, reply.exceptions);
  cache.setQueryData(['subscription-choice', id], reply.access.subscription);
  cache.setQueryData<Node>(nodeKey(id), (before) => before === undefined ? before : { ...before, access: reply.access });
  threadWriter.patch(id, (before) => ({ ...before, access: reply.access }));
  attentionWriter.level(id, reply.access.subscription);
}
