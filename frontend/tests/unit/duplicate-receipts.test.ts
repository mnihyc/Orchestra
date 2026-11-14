/** Raw accepted projections stay successful; the full-content helper must not unwrap them first. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { duplicateDocument } from '@/api/duplicates';
import { forkNow, planThreadMoveOut, startFork, startThreadMoveOut } from '@/api/forks';
import { findAct } from '@/api/acts';
const calls = vi.hoisted(() => ({ post: vi.fn<(path: string, options: unknown) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, POST: calls.post } }; });
vi.mock('@/api/trace', () => ({ traceJson: vi.fn() }));
beforeEach(() => vi.clearAllMocks());
it('opens the actual document identity from a native content-free tree receipt without readback', async () => {
  calls.post.mockResolvedValue({ data: { projection: 'receipt', operation: 'copy-operation', entries: [{ kind: 'document', id: 'made-document' }] }, response: new Response(null, { status: 200 }) });
  expect(await duplicateDocument('source', { kind: 'container', id: 'source-folder' }, undefined, 'copy-key')).toEqual({ kind: 'document', id: 'made-document' });
  expect(calls.post).toHaveBeenCalledExactlyOnceWith('/document-tree/operations', expect.objectContaining({ body: { command: 'copy', items: [{ kind: 'document', id: 'source' }], to: { kind: 'container', id: 'source-folder' }, name: null, client_id: 'copy-key', version: null, deletion_plan: null } }));
});
it('returns a sparse immediate-fork acknowledgement as accepted, with no invented Job', async () => {
  const data = { projection: 'receipt', job_id: 'accepted-job', thread_id: 'reserved-thread' };
  calls.post.mockResolvedValue({ data, response: new Response(null, { status: 202 }) });
  expect(await forkNow('source', 'fork-key')).toEqual(data); expect(calls.post).toHaveBeenCalledOnce();
});
it('a planned copy also keeps the native accepted job identity without a full Job read', async () => {
  const data = { projection: 'receipt', job_id: 'accepted-job' };
  calls.post.mockResolvedValue({ data, response: new Response(null, { status: 202 }) });
  expect(await startFork('source', { preflight_id: 'native-plan' }, 'fork-key')).toEqual(data); expect(calls.post).toHaveBeenCalledOnce();
});

it('reviews and confirms a thread move as one native copy-and-delete job without a browser deletion', async () => {
  const original = { subject: { kind: 'thread', id: 'source' }, destination: { kind: 'node', id: 'other-topic' }, revision: 3, stamp: 'source-footprint', effects: ['remove_source'] };
  const plan = { source: { id: 'source' }, preflight_id: 'native-move-plan' };
  calls.post.mockResolvedValueOnce({ data: { plan, delete_plan: original, notification: null, open_side_threads: [] }, response: new Response(null, { status: 200 }) });
  const request = { result: 'new' as const, topic_id: 'other-topic', history: { mode: 'all' as const }, access: 'defaults' as const };
  const reviewed = await planThreadMoveOut('source', request);
  expect(reviewed.original.deletePlan).toEqual(original);
  calls.post.mockResolvedValueOnce({ data: { projection: 'receipt', job_id: 'accepted-move' }, response: new Response(null, { status: 202 }) });
  expect(await startThreadMoveOut('source', { preflight_id: reviewed.preflight_id, notify: false }, 'move-key')).toEqual({ projection: 'receipt', job_id: 'accepted-move' });
  expect(calls.post.mock.calls.map(([path]) => path)).toEqual(['/threads/{id}/move-out/preview', '/threads/{id}/move-out']);
  expect(calls.post.mock.calls[1]?.[1]).toEqual(expect.objectContaining({ body: { preflight_id: 'native-move-plan', notify: false } }));
  expect(findAct('POST', '/threads/source/move-out/preview')?.changes).toEqual([]);
  expect(findAct('POST', '/threads/source/move-out')).not.toBeNull();
});

it('refuses a thread move review that names a different original or destination', async () => {
  const response = { plan: { source: { id: 'source' }, preflight_id: 'move' }, delete_plan: { subject: { kind: 'thread', id: 'foreign' }, destination: { kind: 'node', id: 'other-topic' }, revision: 1, stamp: 'stamp', effects: [] }, notification: null, open_side_threads: [] };
  calls.post.mockResolvedValue({ data: response, response: new Response(null, { status: 200 }) });
  await expect(planThreadMoveOut('source', { result: 'new', topic_id: 'other-topic' })).rejects.toThrow('did not match');
});
