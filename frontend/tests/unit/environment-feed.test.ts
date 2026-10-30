/** What an environment's own feed changes on its screen: the environment a frame carries takes the place of the one held, a frame of jobs alone leaves it, and being followed anew reads it again. */
import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';

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
