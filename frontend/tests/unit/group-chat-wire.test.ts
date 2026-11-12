/** Native group additions stay on the same client and preserve missing-name versus explicit-null semantics. */
import type { HttpTransport } from '../doubles/http-wire';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ReceiptAcknowledged } from '@/api/client';
import { createChat, joinGroup, setGroupManager, updateGroup } from '@/api/chats';
const wire = vi.hoisted(() => ({ patch: vi.fn<HttpTransport>(), put: vi.fn<HttpTransport>(), post: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<typeof Client>(), api: { PATCH: wire.patch, PUT: wire.put, POST: wire.post } }));
beforeEach(() => { vi.clearAllMocks(); wire.patch.mockResolvedValue({ data: { id: 'group' }, response: new Response(null, { status: 200 }) }); wire.put.mockResolvedValue({ data: { id: 'group' }, response: new Response(null, { status: 200 }) }); wire.post.mockResolvedValue({ response: new Response(null, { status: 204 }) }); });
it('sends actual home and visibility without renaming when name is absent', async () => {
  await updateGroup('group', { home_id: 'topic', visibility: 'public' });
  expect(wire.patch).toHaveBeenCalledWith('/chats/{id}', { params: { path: { id: 'group' } }, body: { home_id: 'topic', visibility: 'public' } });
  await updateGroup('group', { name: null, home_id: null });
  expect(wire.patch).toHaveBeenLastCalledWith('/chats/{id}', { params: { path: { id: 'group' } }, body: { name: null, home_id: null } });
});
it('uses native manager and join operations without a fake full reply', async () => {
  await setGroupManager('group', 'person', true); await joinGroup('group');
  expect(wire.put).toHaveBeenCalledWith('/chats/{id}/members/{user_id}/manager', { params: { path: { id: 'group', user_id: 'person' } }, body: { manager: true } });
  expect(wire.post).toHaveBeenCalledWith('/chats/{id}/join', { params: { path: { id: 'group' } } });
});
it('retains the native accepted receipt instead of treating it as a Chat', async () => {
  wire.patch.mockResolvedValue({ data: { projection: 'receipt', chat_id: 'group' }, response: new Response(null, { status: 200 }) });
  await expect(updateGroup('group', { visibility: 'private' })).rejects.toBeInstanceOf(ReceiptAcknowledged);
});
it('passes explicit group intent through the existing creation and retained key', async () => {
  wire.post.mockResolvedValue({ data: { id: 'group' }, response: new Response(null, { status: 201 }) });
  await createChat('root', { user_ids: ['person'], home_id: 'topic', visibility: 'private' }, 'create-key');
  expect(wire.post).toHaveBeenCalledWith('/orgs/{id}/chats', { params: { path: { id: 'root' } }, body: { user_ids: ['person'], home_id: 'topic', visibility: 'private' }, headers: { 'Idempotency-Key': 'create-key' } });
});
