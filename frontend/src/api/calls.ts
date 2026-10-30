/** The current recovery and explicit controls, with ordinary thread execution fencing. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { threadAct } from '@/api/threadRead';
import { threadReadTarget, type ThreadReadTarget } from '@/realtime/threadScope';
import type { CallRecoveryAction, ThreadCallRecoveryReply } from '@/generated/api-types';

export const callRecoveryKey = (id: string) => ['call-recovery', id] as const;
export const callRecoveryQuery = (id: string) => queryOptions({
  queryKey: callRecoveryKey(id),
  queryFn: ({ signal }): Promise<ThreadCallRecoveryReply> => ok(api.GET('/threads/{id}/calls/recovery', { params: { path: { id } }, signal })),
  staleTime: Infinity,
  refetchOnMount: 'always',
});

export function controlCallRecovery(target: ThreadReadTarget, action: CallRecoveryAction): Promise<ThreadCallRecoveryReply> {
  const id = threadReadTarget(target).threadId;
  return threadAct(target, () => ok(api.POST('/threads/{id}/calls/recovery', { params: { path: { id } }, body: { action } })));
}
