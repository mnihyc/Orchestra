/** What an environment's own feed changes on its screen: the environment a frame carries takes the place of the one held, a frame of jobs alone leaves it, and being followed anew reads it again. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { describe, expect, it, vi } from 'vitest';

import { environmentKey } from '@/api/environments';
import type { Environment } from '@/generated/api-types';
import { applyEnvironmentFrame, environmentFeed } from '@/realtime/environmentFeed';
import type { FeedFrame } from '@/realtime/socket';

const ID = '0b6a3c1e-5d2f-4c8e-9a71-2f3e4d5c6b7a';

const environment = (status: Environment['status']): Environment => ({ id: ID, name: 'Workspace', status }) as unknown as Environment;

const sync = (prev: number, data: Record<string, unknown>): FeedFrame =>
  ({ op: 'sync', id: 1, feed: environmentFeed(ID), prev, seq: prev + 1, data }) as unknown as FeedFrame;

function held(client: QueryClient): Environment | undefined {
  return client.getQueryData<Environment>(environmentKey(ID));
}

describe("an environment's own feed", () => {
  it('is named for the environment', () => {
    expect(environmentFeed(ID)).toBe(`env:${ID}`);
  });

  it('puts the environment a frame carries in the place of the one the screen holds', () => {
    const client = new QueryClient();
    client.setQueryData(environmentKey(ID), environment('running'));
    applyEnvironmentFrame(client, ID, sync(4, { environment: environment('stopped'), jobs: [] }));
    expect(held(client)?.status).toBe('stopped');
  });

  it('leaves what is held when a frame carries no environment and never invalidates it on subscription news', () => {
    const client = new QueryClient();
    client.setQueryData(environmentKey(ID), environment('running'));
    applyEnvironmentFrame(client, ID, sync(5, { jobs: [] }));
    expect(held(client)?.status).toBe('running');
    expect(client.getQueryState(environmentKey(ID))?.isInvalidated).toBe(false);
    applyEnvironmentFrame(client, ID, { op: 'resync', id: 1, feed: environmentFeed(ID) } as unknown as FeedFrame);
    expect(client.getQueryState(environmentKey(ID))?.isInvalidated).toBe(false);
  });
});

it('retakes open native configuration rows when their discovered names change', async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rows = vi.fn(() => Promise.resolve('current configurations'));
  const other = vi.fn(() => Promise.resolve('other configurations'));
  const key = ['environment-mcp', ID, 'thread'];
  const otherKey = ['environment-mcp', 'another-environment', 'thread'];
  client.setQueryData(environmentKey(ID), { ...environment('running'), mcp_found: [] });
  client.setQueryData(key, 'old configurations'); client.setQueryData(otherKey, 'old other configurations');
  const stop = new QueryObserver(client, { queryKey: key, queryFn: rows, staleTime: Infinity }).subscribe(() => undefined);
  const stopOther = new QueryObserver(client, { queryKey: otherKey, queryFn: other, staleTime: Infinity }).subscribe(() => undefined);
  try {
    const current = { ...environment('running'), mcp_found: [{ name: 'docs', source: 'codex', scope: 'user', directory: null, transport: 'stdio', imported: null }] };
    applyEnvironmentFrame(client, ID, sync(10, { environment: current, jobs: [] }));
    await vi.waitFor(() => expect(rows).toHaveBeenCalledOnce());
    expect(other).not.toHaveBeenCalled();
    applyEnvironmentFrame(client, ID, sync(11, { environment: current, jobs: [] }));
    await Promise.resolve();
    expect(rows).toHaveBeenCalledOnce();
  } finally { stop(); stopOther(); client.clear(); }
});
