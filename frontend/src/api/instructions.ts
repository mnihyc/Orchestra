/** What a thread's agent reads as its instructions, source by source, as the reader may see them. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ThreadInstructions } from '@/generated/api-types';

export const threadInstructionsQuery = (thread: string) => queryOptions({
  queryKey: ['thread-instructions', thread] as const,
  queryFn: ({ signal }): Promise<ThreadInstructions> => ok(api.GET('/threads/{id}/instructions', { params: { path: { id: thread } }, signal })),
  retry: false,
});
