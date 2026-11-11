/** Pending lines consume their admitted opening subset, never the complete topic-list cache. */
import { QueryClient, QueryClientProvider, QueryObserver } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { feedSequences } from '@/api/cache';
import { pendingAgentSchedulesQuery, seedPendingAgentSchedules, topicSchedulesQuery, type ThreadPendingAgentSchedules } from '@/api/inputSchedules';
import { scheduledReads } from '@/api/readScheduler';
import { PendingAgentSchedules } from '@/features/schedules/Schedules';
import { i18n, setLocale } from '@/i18n';
import type { HttpTransport } from '../doubles/http-wire';
import { inputSchedule, SCHEDULE_THREAD, SCHEDULE_TOPIC } from '../doubles/input-schedules';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof Client>(), api: { GET: wire.get } }));
const session = client.session.getState();
const caches: QueryClient[] = [];
const thread = () => ({ ...snapshot().thread, id: SCHEDULE_THREAD, topic_id: SCHEDULE_TOPIC });
const ready = (seq = 10, name = 'Check the CI run'): ThreadPendingAgentSchedules => {
  const row = inputSchedule();
  return { seq, items: [{ id: row.id, name, state: 'active', next_run_at: '2030-01-01T21:30:00Z' }] };
};
function cache() {
  const value = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  caches.push(value); return value;
}
const opening = (data: unknown) => ({ data: { sync: { thread: thread() }, pending_agent_schedules: { status: 'ready', data } }, response: new Response() });
beforeEach(() => {
  scheduledReads.advance(); vi.clearAllMocks();
  client.session.setState({ userId: thread().owner.id, readerVersion: 901 });
  wire.get.mockImplementation(() => Promise.resolve(opening(ready())));
});
afterEach(async () => {
  cleanup(); caches.splice(0).forEach((one) => one.clear()); scheduledReads.advance();
  feedSequences.delete(`topic:${SCHEDULE_TOPIC}`); client.session.setState(session); await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`renders supplied pending lines without an HTTP read or a fabricated complete list in ${locale}`, async () => {
    await setLocale(locale); const held = cache();
    seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: ready() });
    show(<QueryClientProvider client={held}><PendingAgentSchedules thread={thread()} /></QueryClientProvider>);
    const lines = await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') });
    expect(within(lines).getByText('Check the CI run')).toBeVisible();
    expect(held.getQueryData(topicSchedulesQuery(SCHEDULE_TOPIC).queryKey)).toBeUndefined();
    expect(wire.get).not.toHaveBeenCalled();
  });

  it(`keeps an independently failed projection visible until an explicit opening retry in ${locale}`, async () => {
    await setLocale(locale); const held = cache();
    seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'failed', retry_after: null,
      problem: { type: 'orch:error:internal', code: 'internal', title: '', detail: '', status: 503, request_id: '' } });
    show(<QueryClientProvider client={held}><PendingAgentSchedules thread={thread()} /></QueryClientProvider>);
    expect(await screen.findByRole('alert')).toBeVisible(); expect(wire.get).not.toHaveBeenCalled();
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText('Check the CI run')).toBeVisible();
    expect(wire.get).toHaveBeenCalledOnce();
    expect(wire.get.mock.calls[0]?.[0]).toBe('/threads/{id}/opening');
    expect(held.getQueryData(topicSchedulesQuery(SCHEDULE_TOPIC).queryKey)).toBeUndefined();
  });

  it(`does not read or show current pending work in a retained view in ${locale}`, async () => {
    await setLocale(locale); const held = cache();
    seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: ready() });
    show(<QueryClientProvider client={held}><PendingAgentSchedules thread={thread()} readOnly /></QueryClientProvider>);
    expect(screen.queryByText('Check the CI run')).toBeNull(); expect(wire.get).not.toHaveBeenCalled();
  });

  it(`preserves an access refusal without silently borrowing the topic reader in ${locale}`, async () => {
    await setLocale(locale); const held = cache();
    seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'failed', retry_after: null,
      problem: { type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' } });
    show(<QueryClientProvider client={held}><PendingAgentSchedules thread={thread()} /></QueryClientProvider>);
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).toBeNull();
    expect(wire.get).not.toHaveBeenCalled();
    expect(held.getQueryData(topicSchedulesQuery(SCHEDULE_TOPIC).queryKey)).toBeUndefined();
  });

  it(`rejects malformed and foreign-thread retry answers without showing fabricated empty work in ${locale}`, async () => {
    await setLocale(locale); const held = cache();
    seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: { seq: 10, items: [{ id: 'partial' }] } });
    show(<QueryClientProvider client={held}><PendingAgentSchedules thread={thread()} /></QueryClientProvider>);
    expect(await screen.findByRole('alert')).toBeVisible(); expect(wire.get).not.toHaveBeenCalled();
    wire.get.mockResolvedValueOnce({ data: { ...opening(ready()).data, sync: { thread: { ...thread(), id: 'other-thread' } } }, response: new Response() });
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(held.getQueryState(pendingAgentSchedulesQuery(SCHEDULE_THREAD, SCHEDULE_TOPIC).queryKey)?.error?.message).toContain('another thread'));
    expect(screen.queryByText('Check the CI run')).toBeNull(); expect(screen.getByRole('alert')).toBeVisible();
  });
}

it('keeps a newer native cut when an abort-ignoring opening reply arrives late and settles its flight', async () => {
  const held = cache(), key = pendingAgentSchedulesQuery(SCHEDULE_THREAD, SCHEDULE_TOPIC).queryKey;
  let finish: ((value: ReturnType<typeof opening>) => void) | undefined;
  wire.get.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const observer = new QueryObserver(held, pendingAgentSchedulesQuery(SCHEDULE_THREAD, SCHEDULE_TOPIC));
  const off = observer.subscribe(() => undefined);
  await waitFor(() => expect(wire.get).toHaveBeenCalledOnce());
  act(() => seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: ready(12, 'New native work') }));
  const settle = finish; if (settle === undefined) throw new Error('The native retry did not start');
  await act(async () => { settle(opening(ready(10, 'Old work'))); await Promise.resolve(); });
  expect(held.getQueryData(key)).toEqual(ready(12, 'New native work'));
  expect(held.getQueryState(key)?.fetchStatus).toBe('idle');
  seedPendingAgentSchedules(held, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: ready(11, 'Older supplied work') });
  expect(held.getQueryData(key)).toEqual(ready(12, 'New native work')); off();
});
