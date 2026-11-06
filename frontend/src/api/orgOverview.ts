/** The initial overview owns native query pages; explicit controls keep their native readers. */
import { CancelledError, queryOptions, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { ApiError, api, client, ok } from '@/api/client';
import { outsideStatusQuery } from '@/api/away';
import { firstStepsKey } from '@/api/firstSteps';
import { activityQuery, childrenQuery, listedPage, nodeKey, orgBySlugKey } from '@/api/nodes';
import { outsideOpeningsQuery } from '@/api/ports';
import { namesQuery, NAME_LIMIT_MAX } from '@/api/search';
import { usageQuery, type UsageParams } from '@/api/usage';
import { scheduledReads } from '@/api/readScheduler';
import type { Node, OrgOverview, Problem } from '@/generated/api-types';

export interface OverviewParams { participating: boolean; usage: UsageParams }
export const orgOverviewKey = (id: string, params: OverviewParams) => ['org-overview', id, params] as const;

type Part<T> = { status: 'ready'; data: T } | { status: 'failed'; problem: Problem; retry_after?: number | null } | { status: 'omitted' };

export function hydrateOverview(cache: QueryClient, key: QueryKey, data: OrgOverview, params: OverviewParams): void {
  const keys: QueryKey[] = [];
  const put = (queryKey: QueryKey, value: unknown) => { keys.push(queryKey); cache.setQueryData(queryKey, value); };
  const section = <T,>(queryKey: QueryKey, value: Part<T>, project: (value: T) => unknown = (value) => value) => {
    if (value.status === 'omitted') return;
    keys.push(queryKey);
    if (value.status === 'ready') cache.setQueryData(queryKey, project(value.data));
    else cache.getQueryCache().build(cache, { queryKey }).setState({ data: undefined, error: new ApiError(value.problem, value.retry_after === null || value.retry_after === undefined ? null : new Date(Date.now() + value.retry_after * 1000)), status: 'error', fetchStatus: 'idle' });
  };
  const { org } = data;
  put(nodeKey(org.id), org);
  put(orgBySlugKey(org.address), org);
  put(childrenQuery(org.id, 'exclude', params.participating).queryKey, { pages: [listedPage(data.children)], pageParams: [null] });
  put(namesQuery({ q: '', kinds: ['thread'], orgId: org.org_id, limit: NAME_LIMIT_MAX }).queryKey, data.recent);
  section(firstStepsKey(org.id), data.first_steps);
  section(outsideStatusQuery(org.id).queryKey, data.outside_status);
  section(usageQuery(params.usage).queryKey, data.usage);
  section(activityQuery(org.id).queryKey, data.activity, (page) => ({ pages: [page], pageParams: [null] }));
  section(outsideOpeningsQuery(org.id).queryKey, data.outside_openings, (page) => ({ pages: [page], pageParams: [null] }));
  scheduledReads.supply(key, keys);
}

export const orgOverviewQuery = (id: string, params: OverviewParams) => queryOptions({
  queryKey: orgOverviewKey(id, params),
  queryFn: ({ client: cache }): Promise<OrgOverview> => scheduledReads.run(orgOverviewKey(id, params), async (current) => {
    const reader = client.session.getState().readerVersion;
    const reply = await ok(api.GET('/orgs/{id}/overview', { params: { path: { id }, query: {
      participating: params.participating, from: params.usage.from, to: params.usage.to,
      usage_by: params.usage.breakdowns[1] ?? 'model',
    } } }));
    if (!current() || reader !== client.session.getState().readerVersion) throw new CancelledError({ silent: true });
    if (reply.mode !== 'overview') throw new Error('The native overview returned a different selection');
    const data = reply.data;
    hydrateOverview(cache, orgOverviewKey(id, params), data, params);
    return data;
  }),
});

/** A landing's remembered place and organization share one bounded native selector.
 * It reads no unshown overview sections, and preserves per-node ordinary visibility.
 */
export function resolveLandingPlaces(cache: QueryClient, rootId: string, ids: readonly string[]): Promise<Node[]> {
  const selected = [...new Set(ids)];
  if (selected.length === 0) return Promise.resolve([]);
  const key = ['landing-places', rootId, selected] as const;
  const reader = client.session.getState().readerVersion;
  return scheduledReads.run(key, async (current) => {
    const reply = await ok(api.GET('/orgs/{id}/overview', { params: { path: { id: rootId }, query: { resolve_ids: selected.join(',') } } }));
    if (!current() || reader !== client.session.getState().readerVersion) throw new CancelledError({ silent: true });
    if (reply.mode !== 'places') throw new Error('The native place selector returned a different selection');
    const keys: QueryKey[] = [];
    for (const node of reply.nodes) {
      keys.push(nodeKey(node.id)); cache.setQueryData(nodeKey(node.id), node);
      if (node.kind === 'org') { keys.push(orgBySlugKey(node.address)); cache.setQueryData(orgBySlugKey(node.address), node); }
    }
    scheduledReads.supply(key, keys);
    return reply.nodes;
  });
}
