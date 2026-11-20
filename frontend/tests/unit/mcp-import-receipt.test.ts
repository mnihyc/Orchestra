/** A committed import receipt does not require a second import or an invented full server row. */
import { QueryClient } from '@tanstack/react-query';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { environmentMcpKey, environmentMcpQuery, importEnvironmentMcp } from '@/api/environments';
import type { HttpTransport } from '../doubles/http-wire';

const calls = vi.hoisted(() => ({ post: vi.fn<HttpTransport>(), get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: calls.post, GET: calls.get } };
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

it('keeps each thread connection read separate from the environment discovery list', async () => {
  calls.get.mockImplementation((_path, options) => Promise.resolve({ data: {
    applies: 'next_turn', servers: [], import: { allowed: false, reason: 'not_owner' },
    found: [{ name: 'docs', source: 'codex', scope: 'user', directory: null, transport: 'stdio', imported: null,
      state: options.params?.query?.thread_id === 'first' ? 'running' : 'failed' }],
  }, response: new Response(null, { status: 200 }) }));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  try {
    const first = await cache.query(environmentMcpQuery('env', 'first'));
    const second = await cache.query(environmentMcpQuery('env', 'second'));
    await cache.query(environmentMcpQuery('env'));
    expect(first.found[0]?.state).toBe('running');
    expect(second.found[0]?.state).toBe('failed');
    expect(cache.getQueryData(environmentMcpKey('env', 'first'))).toEqual(first);
    expect(environmentMcpKey('env')).not.toEqual(environmentMcpKey('env', 'first'));
    expect(calls.get.mock.calls.map(([, options]) => options.params?.query)).toEqual([{ thread_id: 'first' }, { thread_id: 'second' }, {}]);
  } finally { cache.clear(); }
});
