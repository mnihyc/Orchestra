/** The native node opening supplies its visible section readers before its node is published. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { client } from '@/api/client';
import { projectQuery } from '@/api/nodes';
import { scheduledReads } from '@/api/readScheduler';
import { outsideOpeningsQuery } from '@/api/ports';
import { usageQuery, type UsageParams } from '@/api/usage';
import { ACME } from '../doubles/fixtures';
import { nodeOverview } from '../doubles/screen-openings';
import { World } from '../doubles/world';

const wire = vi.hoisted(() => ({ get: vi.fn<(path: string, options: unknown) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<typeof ClientModule>(), api: { GET: wire.get }, ok: (read: Promise<unknown>) => read }));

it('opens the project once and reuses its exact native usage and admission pages', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const params: UsageParams = { scope: `node:${ACME.web}`, from: '2026-10-01', to: '2026-10-07', breakdowns: [], filters: {} };
  const data = await nodeOverview(world, person, ACME.web, new URLSearchParams({ from: params.from, to: params.to, breakdowns: '' }));
  if (data.usage.status !== 'ready') throw new Error('The native project fixture needs its usage projection');
  const before = client.session.getState();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  wire.get.mockReset().mockResolvedValue(data);
  scheduledReads.advance();
  try {
    client.session.setState({ status: 'signed_in', userId: person.me.id });
    expect(await cache.query(projectQuery(ACME.web, params))).toEqual(data.node);
    expect(await cache.query(usageQuery(params))).toEqual(data.usage.data);
    if (data.outside_openings.status === 'ready') expect((await cache.infiniteQuery(outsideOpeningsQuery(ACME.web))).pages).toEqual([data.outside_openings.data]);
    expect(wire.get.mock.calls.map(([path]) => path)).toEqual(['/nodes/{id}/overview']);
  } finally { cache.clear(); scheduledReads.advance(); client.session.setState(before); }
});
