/** A later chat notice repairs the authorized list preview without opening messages or reading unrelated lists. */
import { QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { heardOfChat } from '@/realtime/stores/chat';
import { ADA, chat } from '../doubles/chat-rows';

afterEach(() => { queryClient.clear(); });
it('refreshes a listed empty conversation on its first message and ignores a repeated notice', async () => {
  const initial = chat({ id: 'notice-preview', last_message_seq: 0, preview: null });
  const page = (row: typeof initial) => ({ pages: [{ items: [row], next_cursor: null }], pageParams: [null] });
  const key = ['chats', 'preview-reader'], otherKey = ['chats', 'unrelated-reader'], inactiveKey = ['chats', 'inactive-preview'];
  queryClient.setQueryData(key, page(initial)); queryClient.setQueryData(inactiveKey, page(initial));
  queryClient.setQueryData(otherKey, page(chat({ id: 'other-preview' })));
  const read = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page({ ...initial, last_message_seq: 1, preview: { id: 'first-message', author: ADA, text: 'The first line', deleted: false } }));
  const otherRead = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page(chat({ id: 'other-preview' })));
  const observer = new QueryObserver(queryClient, { queryKey: key, queryFn: read, staleTime: Infinity });
  const other = new QueryObserver(queryClient, { queryKey: otherKey, queryFn: otherRead, staleTime: Infinity });
  const release = observer.subscribe(() => undefined), releaseOther = other.subscribe(() => undefined);
  try {
    heardOfChat(initial.id, 1, '2026-11-11T00:00:00Z');
    await expect.poll(() => queryClient.getQueryData<ReturnType<typeof page>>(key)?.pages[0]?.items[0]?.preview?.text).toBe('The first line');
    expect(read).toHaveBeenCalledTimes(1); expect(otherRead).not.toHaveBeenCalled();
    expect(queryClient.getQueryState(inactiveKey)?.isInvalidated).toBe(true);
    heardOfChat(initial.id, 1, '2026-11-11T00:00:00Z');
    expect(read).toHaveBeenCalledTimes(1);
  } finally { release(); releaseOther(); }
});
