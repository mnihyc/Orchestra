/** One current metadata reader per exact native workspace door, sharing the existing cache lifetime. */
import { resultThreadRead, type ThreadReadRef } from '@/realtime/threadScope';

export function workspaceThreadRead(environmentId: string | null, threadId: string | null): ThreadReadRef | null {
  if (environmentId === null || threadId === null) return null;
  // This is a local reader owner, never a native URL or a conversation history selector.
  // No additional registry/map is retained; existing demand, reader rebase and eviction own it.
  return resultThreadRead(threadId, `workspace-source:${JSON.stringify([environmentId, threadId])}`);
}
