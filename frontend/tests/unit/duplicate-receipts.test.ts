/** Raw accepted projections stay successful; the full-content helper must not unwrap them first. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { duplicateDocument } from '@/api/duplicates';
import { forkNow, startFork } from '@/api/forks';
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
