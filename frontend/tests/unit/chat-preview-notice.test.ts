/** A later chat notice repairs the authorized list preview without opening messages or reading unrelated lists. */
import { QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { ApiError } from '@/api/client';
import { chatWriter, heardOfChat, useChatStore } from '@/realtime/stores/chat';
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

for (const refusal of [false, true]) it(`refreshes lists holding a revoked chat and retains authorized inspection after ${refusal ? 'a refused reload' : 'feed closure'}`, async () => {
  const row = chat({ id: 'left-group', kind: 'group' });
  const page = (items: typeof row[]) => ({ pages: [{ items, next_cursor: null }], pageParams: [null] });
  const ownKey = ['chats', 'participating'], adminKey = ['chats', 'inspection'];
  const inactiveKey = ['chats', 'inactive-left-group'], otherKey = ['chats', 'unrelated'];
  for (const key of [ownKey, adminKey, inactiveKey]) queryClient.setQueryData(key, page([row]));
  queryClient.setQueryData(otherKey, page([chat({ id: 'still-in-group' })]));
  const read = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page([]));
  const inspect = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page([row]));
  const otherRead = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page([]));
  const observers = [[ownKey, read], [adminKey, inspect], [otherKey, otherRead]] as const;
  const releases = observers.map(([queryKey, queryFn]) => new QueryObserver(queryClient, { queryKey, queryFn, staleTime: Infinity }).subscribe(() => undefined));
  try {
    chatWriter.reset(row.id, row, [], false, 0);
    if (refusal) chatWriter.failed(row.id, new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'Not found', status: 404, detail: 'No longer readable', request_id: 'closed-chat-test' }, null));
    else chatWriter.closed(row.id, 'revoked');
    await expect.poll(() => queryClient.getQueryData<ReturnType<typeof page>>(ownKey)?.pages[0]?.items).toEqual([]);
    expect(read).toHaveBeenCalledTimes(1);
    expect(inspect).toHaveBeenCalledTimes(1);
    expect(queryClient.getQueryData<ReturnType<typeof page>>(adminKey)?.pages[0]?.items).toEqual([row]);
    expect(queryClient.getQueryState(inactiveKey)?.isInvalidated).toBe(true);
    expect(otherRead).not.toHaveBeenCalled();
    expect(useChatStore.getState().byChat.get(row.id)?.closed).toBe('revoked');
  } finally { releases.forEach((release) => release()); }
});

it('refreshes an existing empty chat list on a new chat notice through its exact query key', async () => {
  const key = ['chats', 'empty-org', { scope: 'mine', of: null }];
  const page = (items: ReturnType<typeof chat>[]) => ({ pages: [{ items, next_cursor: null }], pageParams: [null] });
  queryClient.setQueryData(key, page([]));
  const row = chat({ id: 'new-direct', last_message_seq: 1 });
  const read = vi.fn<() => Promise<ReturnType<typeof page>>>().mockResolvedValue(page([row]));
  const release = new QueryObserver(queryClient, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    heardOfChat(row.id, 1, '2026-11-11T00:00:00Z');
    await expect.poll(() => queryClient.getQueryData<ReturnType<typeof page>>(key)?.pages[0]?.items).toEqual([row]);
    expect(read).toHaveBeenCalledTimes(1);
  } finally { release(); }
});
