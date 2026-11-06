// @vitest-environment node
/** Native observers and authority repair share the overview owner, retaining explicit paging and retry. */
import { InfiniteQueryObserver, QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { firstStepsQuery } from '@/api/firstSteps';
import { childrenQuery } from '@/api/nodes';
import { hydrateOverview, orgOverviewKey, type OverviewParams } from '@/api/orgOverview';
import { capturePeopleHint, repairPeople } from '@/api/peopleChanges';
import { scheduledReads } from '@/api/readScheduler';
import { ACME } from '../doubles/fixtures';
import { orgOverview } from '../doubles/screen-openings';
import { World } from '../doubles/world';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: unknown) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof import('@/api/client')>();
  return { ...actual, api: { ...actual.api, GET: calls.get } };
});

it('mounts the supplied zero-stale setup and child page without HTTP, then repairs only their active aggregate', async () => {
  scheduledReads.advance(); calls.get.mockReset();
  const world = new World(); const person = world.people.get('ada')!;
  const data = await orgOverview(world, person, ACME.org, new URLSearchParams());
  const params: OverviewParams = { participating: false, usage: { scope: `node:${ACME.org}`, from: '2026-11-02', to: '2026-11-06', breakdowns: ['person', 'model'], limit: 10, filters: {} } };
  const key = orgOverviewKey(ACME.org, params);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  cache.setQueryData(key, data); hydrateOverview(cache, key, data, params);
  const aggregate = vi.fn(() => { hydrateOverview(cache, key, data, params); return Promise.resolve(data); });
  const parent = new QueryObserver(cache, { queryKey: key, queryFn: aggregate, staleTime: Infinity });
  const children = new InfiniteQueryObserver(cache, childrenQuery(ACME.org));
  const setup = new QueryObserver(cache, firstStepsQuery(ACME.org));
  const leave = [parent.subscribe(() => undefined), children.subscribe(() => undefined), setup.subscribe(() => undefined)];
  try {
    await Promise.resolve(); expect(calls.get).not.toHaveBeenCalled();
    expect(setup.getCurrentResult().isSuccess).toBe(true);
    const cut = capturePeopleHint(cache); await repairPeople(cache, [], cut);
    expect(aggregate).toHaveBeenCalledOnce(); expect(calls.get).not.toHaveBeenCalled();
    expect(setup.getCurrentResult().isSuccess).toBe(true);
    // A person's explicit Retry retains the native query rather than replaying
    // the aggregate's held value or removing the control.
    calls.get.mockResolvedValue({ data: data.first_steps.status === 'ready' ? data.first_steps.data : undefined, response: new Response(null, { status: 200 }) });
    await setup.refetch(); expect(calls.get.mock.calls[0]?.[0]).toBe('/me/first-steps');
  } finally { for (const stop of leave) stop(); cache.clear(); scheduledReads.release(key); }
});
