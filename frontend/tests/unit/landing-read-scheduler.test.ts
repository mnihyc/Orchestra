/** A cold owned-root entrance resolves both stale memories once before choosing its visible scope. */
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { scheduledReads } from '@/api/readScheduler';
import { rememberPlace } from '@/features/navigation/lastPlace';
import { landingAddress } from '@/routes/index';
import type { MeBootstrapReply, Node } from '@/generated/api-types';
import { bootstrap } from '../doubles/bootstrap';
import { id } from '../doubles/fixtures';
import { World } from '../doubles/world';

const calls = vi.hoisted(() => ({ bootstrap: vi.fn<() => Promise<MeBootstrapReply>>(), get: vi.fn<(path: string, options?: { params?: { query?: { resolve_ids?: string } } }) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/features/shared/Shared', () => ({ SharedScreen: () => null }));
vi.mock('@/shell/bootstrap', () => ({ ensureShellBootstrap: calls.bootstrap }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof import('@/api/client')>(); return { ...actual, api: { ...actual.api, GET: calls.get } }; });
const previous = client.session.getState();
afterEach(() => { queryClient.clear(); scheduledReads.advance(); localStorage.clear(); client.session.setState(previous); });

it.each([false, true])('joins remembered place and org with duplicate=%s in one native selector', async (duplicate) => {
  queryClient.clear(); scheduledReads.advance(); localStorage.clear(); calls.get.mockReset(); calls.bootstrap.mockReset();
  const world = new World(); const data = await bootstrap(world, world.people.get('ada')!);
  const place = id(0x7f, 1), lastOrg = duplicate ? place : id(0x7f, 2);
  data.state = { ...data.state, sidebar: { last_org_id: lastOrg } };
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  rememberPlace({ node_id: place, route: `/project/${place}` });
  calls.bootstrap.mockResolvedValue(data);
  calls.get.mockResolvedValue({ data: { mode: 'places', nodes: [] }, response: new Response(null, { status: 200 }) });
  const prepare = vi.fn<(node: Node) => void>(); const target = await landingAddress(prepare);
  const team = data.orgs.find((org) => org.org_kind === 'team');
  expect(target).toBe(`/org/${team!.address}`);
  expect(prepare).toHaveBeenCalledOnce(); expect(prepare.mock.calls[0]?.[0]).toMatchObject({ id: team!.id });
  expect(calls.get).toHaveBeenCalledOnce(); expect(calls.get.mock.calls[0]?.[0]).toBe('/orgs/{id}/overview');
  expect(calls.get.mock.calls[0]?.[1]?.params?.query?.resolve_ids?.split(',')).toEqual(duplicate ? [place] : [place, lastOrg]);
});
