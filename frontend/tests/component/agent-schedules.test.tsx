/** Opening supplies pending lines; the complete topic list stays independent and cancellation is shared. */
import { QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { LiveQueryClient } from '@/api/query';
import type { HttpTransport } from '../doubles/http-wire';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { meQuery } from '@/api/auth';
import { seedPendingAgentSchedules, type InputSchedule } from '@/api/inputSchedules';
import { scheduledReads } from '@/api/readScheduler';
import { PendingAgentSchedules, ThreadSchedules, SchedulesView } from '@/features/schedules/Schedules';
import { makeTab } from '@/features/workbench/layout';
import type * as ViewOpening from '@/features/workbench/viewOpening';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { inputSchedule, scheduleRun, SCHEDULE_THREAD, SCHEDULE_TOPIC } from '../doubles/input-schedules';
import { me } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), remove: vi.fn<HttpTransport>(), shown: false }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof Client>(), api: { GET: wire.get, DELETE: wire.remove } }));
vi.mock('@/features/settings/Editor', () => ({ SettingsEditor: ({ only }: { only: readonly string[] }) => <div data-testid="schedule-settings">{only.join(',')}</div> }));
vi.mock('@/features/workbench/viewOpening', async (load) => ({ ...await load<typeof ViewOpening>(), useViewShown: () => wire.shown }));
const session = client.session.getState(), appearance = useAppearance.getState();
const thread = () => { const row = snapshot().thread; return { ...row, id: SCHEDULE_THREAD, topic_id: SCHEDULE_TOPIC, access: { ...row.access, permissions: ['thread.view', 'thread.interact'] } }; };
let rows: InputSchedule[];
const caches: LiveQueryClient[] = [];
function showSchedules(ui: ReactNode) {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const reader = client.session.getState().userId;
  if (reader === null) throw new Error('The schedule view needs its authenticated reader');
  // This private cache also needs the current person normally supplied by the authenticated shell.
  cache.setQueryData(meQuery.queryKey, me(1, 'schedule-reader', 'Schedule reader', { id: reader }));
  seedPendingAgentSchedules(cache, SCHEDULE_THREAD, SCHEDULE_TOPIC, { status: 'ready', data: pending() });
  caches.push(cache); return show(<QueryClientProvider client={cache}>{ui}</QueryClientProvider>);
}
beforeEach(() => {
  scheduledReads.advance();
  vi.clearAllMocks(); wire.remove.mockReset(); wire.shown = false; rows = [inputSchedule()];
  client.session.setState({ userId: thread().owner.id, readerVersion: 801 });
  wire.get.mockImplementation((path: string) => {
    if (path === '/topics/{id}/sync') return Promise.resolve({ data: { input_schedules: rows }, response: new Response() });
    if (path === '/threads/{id}/opening') return Promise.resolve({ data: { sync: { thread: thread() }, pending_agent_schedules: { status: 'ready', data: pending() } }, response: new Response() });
    throw new Error(`Unexpected read ${path}`);
  });
  wire.remove.mockImplementation(() => { rows = rows.map((row) => ({ ...row, state: 'deleted', next_run_at: null })); return Promise.resolve({ response: new Response(null, { status: 204 }) }); });
});
afterEach(async () => { cleanup(); caches.splice(0).forEach((cache) => cache.clear()); scheduledReads.advance(); client.session.setState(session); useAppearance.setState(appearance); await setLocale('en'); });

function pending() {
  return { seq: 2, items: rows.flatMap((row) => row.kind === 'agent' && row.agent?.thread_id === SCHEDULE_THREAD
    && row.destination.thread?.id === SCHEDULE_THREAD && row.state === 'active' && row.next_run_at !== null
    ? [{ id: row.id, name: row.name, state: row.state, next_run_at: row.next_run_at }] : []) };
}

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('shows each pending agent line and the same source beside people in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  rows = [inputSchedule({ timing: { kind: 'cron', expression: '30 21 * * *', zone: 'UTC' }, last_run: scheduleRun({ state: 'failed', input_id: null, turn_id: null, started_at: null, reason: 'authz.denied' }) }),
    inputSchedule({ id: 'people-schedule', kind: undefined, agent: undefined, name: 'Daily review', timing: { kind: 'repeat', frequency: 'daily', time: '09:00', zone: 'Europe/Paris', weekdays: [] } }),
    inputSchedule({ id: 'paused', name: 'Paused note', state: 'paused', next_run_at: null }),
    inputSchedule({ id: 'foreign', name: 'Another thread', destination: { thread: { id: 'foreign', kind: 'thread', name: 'Other', redacted: false }, new_thread: false } })];
  showSchedules(<><PendingAgentSchedules thread={thread()} /><ThreadSchedules thread={thread()} /></>);
  const pending = await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') });
  expect(within(pending).getByText(i18n.t('agentSchedules.continueAt'))).toBeVisible();
  expect(within(pending).getByText('Check the CI run')).toBeVisible();
  expect(within(pending).queryByText('Daily review')).toBeNull(); expect(within(pending).queryByText('Paused note')).toBeNull();
  const list = await screen.findByRole('table', { name: i18n.t('agentSchedules.title') });
  expect(within(list).getByText('Daily review')).toBeVisible(); expect(within(list).getByText('Paused note')).toBeVisible();
  expect(within(list).getByText('30 21 * * *')).toBeVisible(); expect(within(list).getByText(i18n.t('agentSchedules.outcomes.failed'))).toBeVisible();
  expect(within(list).queryByText('Another thread')).toBeNull();
  expect(screen.getByTestId('schedule-settings')).toHaveTextContent('threads.agent_schedules,threads.agent_schedule_hours,threads.agent_schedule_runs');
  expect(wire.get).toHaveBeenCalledOnce();
  expect(wire.get.mock.calls[0]?.[0]).toBe('/topics/{id}/sync');
  expect(wire.remove).not.toHaveBeenCalled();
});

it('shares pending cancellation between the timeline and list, leaving accepted input delivery to the ordinary tray', async () => {
  let finish: (() => void) | undefined;
  wire.remove.mockImplementation(() => new Promise((resolve) => { finish = () => { rows = rows.map((row) => ({ ...row, state: 'deleted', next_run_at: null })); resolve({ response: new Response(null, { status: 204 }) }); }; }));
  showSchedules(<><PendingAgentSchedules thread={thread()} /><ThreadSchedules thread={thread()} /></>);
  const pending = await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') }), user = userEvent.setup();
  await user.click(within(pending).getByRole('button', { name: i18n.t('common.cancel') }));
  const other = within(await screen.findByRole('table')).getByRole('button', { name: i18n.t('common.cancel') });
  await waitFor(() => expect(other).toHaveAttribute('aria-disabled', 'true'));
  await user.click(other); expect(wire.remove).toHaveBeenCalledOnce();
  const settle = finish; if (settle === undefined) throw new Error('Cancellation did not start');
  await act(() => { settle(); return Promise.resolve(); });
  await waitFor(() => expect(screen.queryByRole('list', { name: i18n.t('agentSchedules.pending') })).toBeNull());
  expect(wire.remove).toHaveBeenCalledExactlyOnceWith('/input-schedules/{id}', { params: { path: { id: inputSchedule().id } } });
});

it('keeps a refused schedule present with a retryable Cancel', async () => {
  wire.remove.mockRejectedValueOnce(new Error('Native refusal')); showSchedules(<PendingAgentSchedules thread={thread()} />);
  const pending = await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') }), user = userEvent.setup();
  await user.click(within(pending).getByRole('button', { name: i18n.t('common.cancel') }));
  expect(await screen.findByRole('alert')).toBeVisible(); expect(screen.getByText('Check the CI run')).toBeVisible();
  await user.click(within(pending).getByRole('button', { name: i18n.t('common.cancel') }));
  await waitFor(() => expect(screen.queryByRole('list', { name: i18n.t('agentSchedules.pending') })).toBeNull()); expect(wire.remove).toHaveBeenCalledTimes(2);
});

it('blocks a viewer, makes no hidden or retained-view read, and does not refetch for an old cancellation response', async () => {
  const row = thread(); row.access.permissions = ['thread.view'];
  const user = userEvent.setup(); showSchedules(<PendingAgentSchedules thread={row} />);
  const pending = await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') }); const cancel = within(pending).getByRole('button', { name: i18n.t('common.cancel') });
  expect(cancel).toHaveAttribute('aria-disabled', 'true'); await user.click(cancel); expect(wire.remove).not.toHaveBeenCalled();
  cleanup(); wire.get.mockClear();
  showSchedules(<><PendingAgentSchedules thread={row} readOnly /><ThreadSchedules thread={row} readOnly /><SchedulesView tab={makeTab('schedules', { thread: row.id })} area="right" /></>);
  expect(wire.get).not.toHaveBeenCalled(); cleanup();
  let finish: (() => void) | undefined;
  wire.remove.mockImplementation(() => new Promise((resolve) => { finish = () => resolve({ response: new Response(null, { status: 204 }) }); }));
  showSchedules(<PendingAgentSchedules thread={thread()} />); await screen.findByRole('list', { name: i18n.t('agentSchedules.pending') });
  await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
  const settle = finish; if (settle === undefined) throw new Error('Cancellation did not start');
  act(() => client.session.setState({ readerVersion: 802 }));
  await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(1));
  await act(() => { settle(); return Promise.resolve(); });
  expect(wire.get).toHaveBeenCalledTimes(1); expect(screen.queryByText(i18n.t('agentSchedules.cancelled'))).toBeNull();
});

it.each([['en', true], ['zh-CN', true], ['en', false], ['zh-CN', false]] as const)('keeps author cancellation separate from thread control in %s (author %s)', async (locale, author) => {
  await setLocale(locale);
  const viewing = thread(); viewing.access.permissions = ['thread.view'];
  const schedule = inputSchedule({ kind: undefined, agent: undefined, name: 'My future review' }); rows = [schedule];
  client.session.setState({ userId: author ? schedule.author.id : 'another-schedule-reader', readerVersion: 803 });
  showSchedules(<ThreadSchedules thread={viewing} />);
  const table = await screen.findByRole('table', { name: i18n.t('agentSchedules.title') });
  const cancel = within(table).getByRole('button', { name: i18n.t('common.cancel') });
  expect(cancel.getAttribute('aria-disabled') === 'true').toBe(!author);
  await userEvent.click(cancel);
  if (author) {
    await waitFor(() => expect(wire.remove).toHaveBeenCalledOnce());
    expect(wire.remove).toHaveBeenCalledWith('/input-schedules/{id}', { params: { path: { id: inputSchedule().id } } });
  } else expect(wire.remove).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('keeps author cancellation unavailable in a retained read-only view in %s', async (locale) => {
  await setLocale(locale); const schedule = inputSchedule();
  client.session.setState({ userId: schedule.author.id, readerVersion: 804 });
  showSchedules(<ThreadSchedules thread={thread()} readOnly />);
  expect(await screen.findByText(i18n.t('composer.row.readOnly'), { exact: true })).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('common.cancel') })).toBeNull();
  expect(wire.remove).not.toHaveBeenCalled();
});
