/** Scoped nonvisual joins for the inspector, deep links and thread-file dialog. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { fileNameOf, type SavedFile, type ThreadFileQuery } from '@/api/exports';
import { fetchItem, fetchItems, fetchOutput, type ItemWindow, OLDER_ITEMS } from '@/api/threads';
import { assertThreadRead, captureThreadRead, readInThread, threadQueryRead,
  threadReadSelector, type ThreadReadCapture } from '@/api/threadRead';
import type { Item, ThreadItemListReply } from '@/generated/api-types';
import { scopedRawFramesQuery } from '@/realtime/rawQuery';
import { sameThreadRead, threadReadTarget, type ThreadReadTarget, threadReadChanged } from '@/realtime/threadScope';

/** The raw transport admits one exact source before its captured HTTP pages start. */
export const viewedRawFramesQuery = scopedRawFramesQuery;

/** Quotes and anchors bind both their request and key to the selected read, not only an item id. */
export const viewedItemQuery = (target: ThreadReadTarget, itemId: string) => {
  const ref = threadReadTarget(target);
  const threadId = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-item', threadId, itemId]);
  return queryOptions({ queryKey: reading.key, enabled: reading.enabled,
    queryFn: async ({ signal }): Promise<Item> => {
      const captured = await reading.capture(signal);
      return fetchItem(ref, itemId, signal, captured);
    }, staleTime: Infinity });
};
export const viewedOutputQuery = (target: ThreadReadTarget, itemId: string, offset: number, limit: number) => {
  const ref = threadReadTarget(target);
  const threadId = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-output', threadId, itemId, offset, limit]);
  return queryOptions({ queryKey: reading.key, enabled: reading.enabled,
    queryFn: async ({ signal }): Promise<string> => {
      const captured = await reading.capture(signal);
      return fetchOutput(ref, itemId, offset, limit, signal, captured);
    }, staleTime: Infinity });
};
export const viewedItemsQuery = (target: ThreadReadTarget, window: ItemWindow) => {
  const ref = threadReadTarget(target);
  const threadId = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-items', threadId, window]);
  return queryOptions({ queryKey: reading.key, enabled: reading.enabled,
    queryFn: async ({ signal }): Promise<ThreadItemListReply> => {
      const captured = await reading.capture(signal);
      assertThreadRead(captured, signal);
      return fetchItems(ref, window, OLDER_ITEMS, signal);
    }, staleTime: Infinity });
};
/** The caller may capture when opening the dialog; switching views must not change that export. */
export async function exportViewedThreadFile(target: ThreadReadTarget, query: ThreadFileQuery, signal?: AbortSignal,
  captured: ThreadReadCapture = captureThreadRead(target)): Promise<SavedFile> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  if (!sameThreadRead(captured.ref,ref)) throw threadReadChanged();
  return readInThread(captured, async (readSignal) => {
    const answer = await api.GET('/threads/{id}/export', {
      params: { path: { id }, query: { format: query.format, detail: query.detail, include: query.include.join(','),
        ...threadReadSelector(captured), ...(query.from === undefined ? {} : { from: query.from }),
        ...(query.to === undefined ? {} : { to: query.to }) } }, parseAs: 'blob',
      signal: readSignal,
    });
    const blob = await ok(Promise.resolve(answer));
    return { blob, name: fileNameOf(answer.response, `thread.${query.format === 'json' ? 'json' : 'md'}`),
      losses: (answer.response.headers.get('X-Orch-Export-Losses') ?? '').split(',').map((loss) => loss.trim()).filter((loss) => loss !== '') };
  }, signal);
}
