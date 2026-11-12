/** Link-origin operands stay on existing gifts, with the actual actor left to the server. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { grantPerson } from '@/api/personActions';
import { shareWithAgents } from '@/api/share';
import type { AgentGift } from '@/features/sharing/linkPlan';
import { pageFor } from './general-access-fixtures';
import { bodyOf, type HttpTransport } from '../doubles/http-wire';

const calls = vi.hoisted(() => ({ patch: vi.fn<HttpTransport>(), post: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof Client>();
  return { ...actual, api: { ...actual.api, PATCH: calls.patch, POST: calls.post } };
});
const origin = { kind: 'chat' as const, chat_id: 'composing-chat' };
beforeEach(() => {
  vi.clearAllMocks();
  calls.patch.mockResolvedValue({ data: pageFor('document'), response: new Response(null, { status: 200 }) });
  calls.post.mockResolvedValue({ response: new Response(null, { status: 204 }) });
});

it('serializes origin on a normal page gift without adding an actor or a notification', async () => {
  await grantPerson({ kind: 'document', id: 'document' }, 'person', 'full_view', false, 'gift-key', origin);
  expect(calls.patch).toHaveBeenCalledWith('/{object}/{id}/permissions', expect.objectContaining({
    params: { path: { object: 'documents', id: 'document' } },
    body: { name: [{ holders: [{ kind: 'user', id: 'person' }], choice: 'full_view', export: false }], link_origin: origin },
    headers: { 'Idempotency-Key': 'gift-key' },
  }));
  expect(bodyOf(calls.patch.mock.calls[0]).link_origin).not.toHaveProperty('by');
});

it('retains the native chat membership choice on the existing person grant route', async () => {
  await grantPerson({ kind: 'chat', id: 'group' }, 'person', 'full_interact', false, 'membership-key', origin);
  expect(calls.post).toHaveBeenCalledWith('/chats/{id}/permissions/people/{user_id}', expect.objectContaining({
    params: { path: { id: 'group', user_id: 'person' } }, body: { choice: 'full_interact', notify: false, link_origin: origin },
    headers: { 'Idempotency-Key': 'membership-key' },
  }));
});

it('keeps ordinary non-link gifts byte-compatible when no origin is supplied', async () => {
  await grantPerson({ kind: 'document', id: 'document' }, 'person', 'full_view', false, 'ordinary-key');
  expect(bodyOf(calls.patch.mock.calls[0])).not.toHaveProperty('link_origin');
});

it('keeps structured origin on the ordinary live agent producer', async () => {
  calls.post.mockResolvedValue({ data: { shares: [] }, response: new Response(null, { status: 200 }) });
  const body: AgentGift = { client_id: 'agent-key', mode: 'live', what: { document_id: 'document' }, to: [{ thread_id: 'recipient' }], parts: { documents: 'read' }, link_origin: { kind: 'thread_input', thread_id: 'composing-thread' } };
  const key = '01950000-0000-7000-8000-000000000072';
  await shareWithAgents(body, key);
  expect(calls.post).toHaveBeenCalledWith('/share/agents', { body, headers: { 'Idempotency-Key': key } });
  expect(bodyOf(calls.post.mock.calls[0]).link_origin).not.toHaveProperty('by');
});
