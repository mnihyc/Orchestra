/** Native schedule writes accept opaque receipts and never dispatch an immediate input. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { createInputSchedule, updateInputSchedule } from '@/api/inputSchedules';
import type { InputScheduleCreateBody } from '@/generated/api-types';
const calls = vi.hoisted(() => ({ post: vi.fn<(path: string, options: unknown) => Promise<{ data: unknown; response: Response }>>(), patch: vi.fn<(path: string, options: unknown) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, POST: calls.post, PATCH: calls.patch } }; });
const receipt = { projection: 'receipt', schedule_id: 'schedule', revision: 2, topic_seq: 8 };
const body: InputScheduleCreateBody = { name: null, plan: false, delivery: 'queue', target: { kind: 'thread', thread_id: 'thread' }, content: [{ type: 'text', text: 'Run the audit' }], native_action: null, timing: { kind: 'once', at: '2030-01-01T09:00:00Z', zone: 'UTC' } };
beforeEach(() => { vi.clearAllMocks(); calls.post.mockResolvedValue({ data: receipt, response: new Response(null, { status: 201 }) }); calls.patch.mockResolvedValue({ data: receipt, response: new Response(null, { status: 200 }) }); });
it('saves the complete definition through the topic operation and keeps a sparse acknowledgement successful', async () => {
  expect(await createInputSchedule('topic', body, 'same-attempt')).toEqual(receipt);
  expect(calls.post).toHaveBeenCalledExactlyOnceWith('/topics/{id}/input-schedules', expect.objectContaining({ params: { path: { id: 'topic' } }, body, headers: { 'Idempotency-Key': 'same-attempt' } }));
  expect(calls.patch).not.toHaveBeenCalled();
});
it('editing uses the shown revision and does not combine an implicit resume', async () => {
  const edit = { ...body, revision: 7, state: null };
  expect(await updateInputSchedule('schedule', edit, 'one-edit')).toEqual(receipt);
  expect(calls.patch).toHaveBeenCalledWith('/input-schedules/{id}', expect.objectContaining({ params: { path: { id: 'schedule' } }, body: edit }));
  expect(calls.post).not.toHaveBeenCalled();
});
it('resume sends only the native state variant and current revision, preserving the private definition', async () => {
  const resume = { revision: 7, name: null, target: null, content: null, timing: null, state: 'active' as const };
  await updateInputSchedule('schedule', resume, 'one-resume');
  expect(calls.patch).toHaveBeenCalledWith('/input-schedules/{id}', expect.objectContaining({ body: resume }));
  expect(resume).not.toHaveProperty('native_action'); expect(calls.post).not.toHaveBeenCalled();
});
