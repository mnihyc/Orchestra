/** One shell aggregate satisfies its existing readers before they mount. */
import type * as LiveModule from '@/realtime/live';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { adminModeQuery } from '@/api/adminMode';
import { permissionsKey, permissionsQuery } from '@/api/access';
import { notificationPrefsKey, notificationPrefsQuery, notifiersKey, notifiersQuery } from '@/api/notifications';
import { meQuery, metaKey } from '@/api/auth';
import { client } from '@/api/client';
import { ApiError } from '@/api/client';
import { act, fireEvent, waitFor } from '@testing-library/react';
import { Banners } from '@/shell/Banners';
import { i18n } from '@/i18n';
import { mark } from '@/lib/marks';
import type * as ClientModule from '@/api/client';
import { orgsQuery } from '@/api/nodes';
import { favoritesQuery, meStateKey, meStateQuery } from '@/api/personal';
import { queryClient as applicationQueries } from '@/api/query';
import { dispatchPersonalEvent, retirePersonalEvents } from '@/realtime/personalSource';
import { BASELINE_CHECK_AFTER_MS, ensureShellBootstrap, useShellBootstrap } from '@/shell/bootstrap';
import { useSelectedOrg } from '@/shell/org';
import { useChatStates } from '@/realtime/stores/chat';
import { useUserStore } from '@/realtime/stores/user';
import { useAttentionStore } from '@/features/navigation/attention';
import type { MeBootstrapReply } from '@/generated/api-types';
import { bootstrap } from '../doubles/bootstrap';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: { signal?: AbortSignal }) => Promise<unknown>>(), admitted: vi.fn<(baseline?: boolean) => Promise<void>>(), joined: vi.fn<() => boolean>() }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof LiveModule>(), ensurePersonalSource: calls.admitted, personalSourceJoined: calls.joined }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});
let data: MeBootstrapReply;
beforeEach(async () => {
  mark('start/script');
  const world = new World();
  const reader = world.people.get('ada');
  if (reader === undefined) throw new Error('Missing Ada fixture');
  data = await bootstrap(world, reader);
  calls.get.mockReset();
  calls.get.mockResolvedValue(data);
  calls.admitted.mockReset().mockResolvedValue();
  calls.joined.mockReset().mockReturnValue(false);
});
function Readers() {
  useQuery(permissionsQuery);
  useQuery(notificationPrefsQuery);
  useQuery(notifiersQuery);
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
function paintContent() { mark('page/content', { user: data.person.id, reader: String(client.session.getState().readerVersion ?? 0) }); }
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
    expect(data.permission_catalogue).toBeDefined();
    expect(cache.getQueryData(permissionsKey)).toEqual(data.permission_catalogue);
    expect(cache.getQueryData(notificationPrefsKey)).toEqual(data.notification_preferences);
    expect(cache.getQueryData(notifiersKey)).toEqual(data.notifier_catalogue);
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

it('sends the first read beside the join of the person\'s own feed, and applies what was told meanwhile after it', async () => {
  const session = client.session.getState();
  // The feed never joins in this case: the read must not wait for it.
  calls.admitted.mockImplementation(() => new Promise<void>(() => undefined));
  let answer!: (value: unknown) => void;
  calls.get.mockImplementation(() => new Promise((resolve) => { answer = resolve; }));
  const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  const told: boolean[] = [];
  try {
    const loader = ensureShellBootstrap(cache, data.person.id);
    await vi.waitFor(() => { expect(calls.get).toHaveBeenCalledOnce(); });
    expect(calls.admitted).toHaveBeenCalledWith(true);
    // Something told by the feed while the read is out is held until the read is in the cache.
    dispatchPersonalEvent(() => { told.push(cache.getQueryData(metaKey) !== undefined); });
    expect(told).toEqual([]);
    answer(data);
    const opened = await loader;
    expect(opened.person.id).toBe(data.person.id);
    expect(told).toEqual([true]);
    expect(await ensureShellBootstrap(cache, data.person.id)).toEqual(opened);
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
  } finally { retirePersonalEvents(); cache.clear(); client.session.setState(session); }
});

it.each(['nothing was missed', 'a change was missed'] as const)('checks the first read once after the feed has joined: %s', async (case_) => {
  vi.useFakeTimers();
  const session = client.session.getState();
  const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  // The first read is made before the feed has joined; by the check it has.
  calls.joined.mockReset().mockReturnValueOnce(false).mockReturnValue(true);
  const later = { ...data, state: { ...data.state, starredElsewhere: 'a place starred on another device' } } as MeBootstrapReply;
  calls.get.mockReset().mockResolvedValueOnce(data).mockResolvedValueOnce(case_ === 'nothing was missed' ? data : later);
  try {
    await ensureShellBootstrap(cache, data.person.id);
    const held = cache.getQueryState(meStateKey)?.dataUpdateCount;
    expect(calls.get).toHaveBeenCalledOnce();
    paintContent();
    await vi.advanceTimersByTimeAsync(BASELINE_CHECK_AFTER_MS);
    await vi.waitFor(() => { expect(calls.get).toHaveBeenCalledTimes(2); });
    if (case_ === 'nothing was missed') {
      await vi.advanceTimersByTimeAsync(0);
      // What was held already said everything the later read says: nothing was taken again.
      expect(cache.getQueryState(meStateKey)?.dataUpdateCount).toBe(held);
    } else await vi.waitFor(() => { expect(cache.getQueryData(meStateKey)).toEqual(later.state); });
    // However it went, one check was made and no more.
    await vi.advanceTimersByTimeAsync(BASELINE_CHECK_AFTER_MS * 4);
    expect(calls.get).toHaveBeenCalledTimes(2);
  } finally { vi.useRealTimers(); retirePersonalEvents(); cache.clear(); client.session.setState(session); }
});

it.each(['notification preferences', 'public metadata', 'thread positions', 'subscriptions'])('repairs a missed change of %s after the personal feed joins', async (member) => {
  vi.useFakeTimers();
  const session = client.session.getState();
  const attention = useAttentionStore.getState();
  const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  calls.joined.mockReset().mockReturnValueOnce(false).mockReturnValue(true);
  let later: MeBootstrapReply = data;
  if (member === 'notification preferences') {
    const prefs = data.notification_preferences;
    if (prefs === null || prefs === undefined) throw new Error('This bootstrap world must include notification preferences.');
    later = { ...data, notification_preferences: { ...prefs, revision: prefs.revision + 1, approvals_always: !prefs.approvals_always } };
  } else if (member === 'public metadata') later = { ...data, meta: { ...data.meta, name: 'Changed installation' } };
  else if (member === 'thread positions') later = { ...data, read_states: [{ thread_id: '01950000-0000-7000-8000-0000000000bb', last_read_seq: 17 }] };
  else later = { ...data, subscriptions: [{ node_id: '01950000-0000-7000-8000-0000000000bb', level: 'all', source: 'manual' }] };
  calls.get.mockReset().mockResolvedValueOnce(data).mockResolvedValueOnce(later);
  try {
    await ensureShellBootstrap(cache, data.person.id);
    paintContent();
    await vi.advanceTimersByTimeAsync(BASELINE_CHECK_AFTER_MS);
    await vi.waitFor(() => {
      expect(calls.get).toHaveBeenCalledTimes(2);
      expect(cache.getQueryData(metaKey)).toEqual(later.meta);
      expect(cache.getQueryData(notificationPrefsKey)).toEqual(later.notification_preferences);
      expect([...useAttentionStore.getState().read]).toEqual(later.read_states.map((row) => [row.thread_id, row.last_read_seq]));
      expect([...useAttentionStore.getState().levels]).toEqual(later.subscriptions.map((row) => [row.node_id, row.level]));
    });
  } finally { vi.useRealTimers(); retirePersonalEvents(); cache.clear(); client.session.setState(session); useAttentionStore.setState(attention); }
});

it('waits for the addressed content before sending the extra joined baseline check', async () => {
  vi.useFakeTimers();
  const session = client.session.getState();
  const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  calls.joined.mockReset().mockReturnValueOnce(false).mockReturnValue(true);
  try {
    await ensureShellBootstrap(cache, data.person.id);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(calls.get).toHaveBeenCalledOnce();
    paintContent();
    await vi.advanceTimersByTimeAsync(BASELINE_CHECK_AFTER_MS);
    await vi.waitFor(() => { expect(calls.get).toHaveBeenCalledTimes(2); });
  } finally { vi.useRealTimers(); retirePersonalEvents(); cache.clear(); client.session.setState(session); }
});

it('shows a failed joined check and retries that account on the same healthy connection', async () => {
  const session = client.session.getState();
  const cache = new QueryClient({ defaultOptions: applicationQueries.getDefaultOptions() });
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  calls.joined.mockReset().mockReturnValueOnce(false).mockReturnValue(true);
  const later = { ...data, state: { ...data.state, changedInTheGap: true } };
  calls.get.mockReset().mockResolvedValueOnce(data).mockRejectedValueOnce(new ApiError({ type: 'orch:error:internal', code: 'internal', title: 'Unavailable', status: 500, detail: 'Try again later.', request_id: 'baseline-check' }, null)).mockResolvedValueOnce(later);
  try {
    await ensureShellBootstrap(cache, data.person.id);
    const view = show(<Banners />, cache);
    act(paintContent);
    await screen.findByRole('alert', undefined, { timeout: BASELINE_CHECK_AFTER_MS + 5_000 });
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap', '/me/bootstrap']);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(cache.getQueryData(meStateKey)).toEqual(later.state));
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap', '/me/bootstrap', '/me/bootstrap']);
    view.unmount();
  } finally { retirePersonalEvents(); cache.clear(); client.session.setState(session); }
});
