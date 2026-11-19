/** One shell aggregate satisfies its existing readers before they mount. */
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { adminModeQuery } from '@/api/adminMode';
import { meQuery, metaKey } from '@/api/auth';
import { client } from '@/api/client';
import type * as ClientModule from '@/api/client';
import { orgsQuery } from '@/api/nodes';
import { favoritesQuery, meStateQuery } from '@/api/personal';
import { queryClient as applicationQueries } from '@/api/query';
import { useShellBootstrap } from '@/shell/bootstrap';
import { useSelectedOrg } from '@/shell/org';
import { useChatStates } from '@/realtime/stores/chat';
import { useUserStore } from '@/realtime/stores/user';
import type { MeBootstrapReply } from '@/generated/api-types';
import { bootstrap } from '../doubles/bootstrap';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: { signal?: AbortSignal }) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});
let data: MeBootstrapReply;
beforeEach(async () => {
  const world = new World();
  const reader = world.people.get('ada');
  if (reader === undefined) throw new Error('Missing Ada fixture');
  data = await bootstrap(world, reader);
  calls.get.mockReset();
  calls.get.mockResolvedValue(data);
});
function Readers() {
  const person = useQuery(meQuery).data;
  const orgs = useQuery(orgsQuery).data;
  const favorites = useQuery(favoritesQuery).data;
  const state = useQuery(meStateQuery).data;
  const mode = useQuery(adminModeQuery).data;
  return <p>{person?.display_name} · {orgs?.length} · {favorites?.length} · {String(state !== undefined && mode !== undefined)}</p>;
}
function Gate() {
  const query = useShellBootstrap(data.person.id);
  return query.data === undefined ? <p>Loading</p> : <Readers />;
}
it('loads the aggregate once, seeds all shell readers and leaves the Inbox page unloaded', async () => {
  const session = client.session.getState();
  const user = useUserStore.getState();
  const chats = useChatStates.getState();
  const selected = useSelectedOrg.getState();
  try {
    client.session.setState({ status: 'signed_in', userId: data.person.id });
    useUserStore.setState({ loaded: false, rows: new Map(), reachedEnd: false });
    // The application uses live freshness; a zero-stale test client would deliberately refetch seeded rows.
    const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
    const content = <QueryClientProvider client={cache}><Gate /></QueryClientProvider>;
    const view = show(content);
    expect(await screen.findByText(`${data.person.display_name} · ${data.orgs.length} · ${data.favorites.length} · true`)).toBeVisible();
    view.rerender(content);
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
    expect(cache.getQueryData(metaKey)).toEqual(data.meta);
    expect(useChatStates.getState().loaded).toBe(true);
    expect(useUserStore.getState().counters).toEqual({ unread: data.notification_summary.unread, approvalsPending: data.notification_summary.approvals_pending });
    expect(useUserStore.getState().loaded).toBe(false);
    expect(useUserStore.getState().rows.size).toBe(0);
    expect(useUserStore.getState().reachedEnd).toBe(false);
  } finally {
    client.session.setState(session);
    useUserStore.setState(user);
    useChatStates.setState(chats);
    useSelectedOrg.setState(selected);
  }
});
