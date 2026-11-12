/** A committed import receipt does not require a second import or an invented full server row. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { importEnvironmentMcp } from '@/api/environments';
import type { HttpTransport } from '../doubles/http-wire';

const calls = vi.hoisted(() => ({ post: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: calls.post } };
});
beforeEach(() => vi.clearAllMocks());
it('keeps the authoritative server id from a restricted accepted receipt', async () => {
  calls.post.mockResolvedValue({ data: { projection: 'receipt', environment_id: 'env', server_id: 'server' }, response: new Response(null, { status: 201 }) });
  const body = { source: 'codex', scope: 'user' as const, directory: null, name: 'docs', enable: true };
  expect(await importEnvironmentMcp('env', body, 'import-key')).toEqual({ id: 'server', server: null });
  expect(calls.post).toHaveBeenCalledOnce();
  expect(calls.post).toHaveBeenCalledWith('/environments/{id}/mcp/imports', {
    params: { path: { id: 'env' } }, body, headers: { 'Idempotency-Key': 'import-key' },
  });
});
