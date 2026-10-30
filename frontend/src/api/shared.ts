/** Things shared by name whose parent the caller cannot see, as the server admits them. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { SharedListItem, SharedListReply } from '@/generated/api-types';

export const sharedKey = ['shared-with-me'] as const;

/** Follow every server page, even when a scanned page contains no readable shares. */
export async function fetchShared(signal?: AbortSignal): Promise<SharedListItem[]> {
  const rows: SharedListItem[] = [];
  let cursor: string | null = null;
  do {
    const page: SharedListReply = await ok(api.GET('/shared', { params: { query: cursor === null ? {} : { cursor } }, ...(signal ? { signal } : {}) }));
    rows.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor !== null);
  return rows;
}

export const sharedQuery = queryOptions({ queryKey: sharedKey,
  queryFn: ({ signal }): Promise<SharedListItem[]> => fetchShared(signal),
  retry: false,
});

/** One visible page; an empty candidate page may still have a continuation. */
export const sharedPageQuery = (cursor: string | null) => queryOptions({
  queryKey: [...sharedKey, cursor],
  queryFn: ({ signal }): Promise<SharedListReply> => ok(api.GET('/shared', { params: { query: cursor === null ? {} : { cursor } }, signal })),
  retry: false,
});
