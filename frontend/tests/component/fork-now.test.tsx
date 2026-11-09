/** R1 W09: the source remains open until a successful live result, then its fork opens exactly once. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Forks from '@/api/forks';
import { client } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { useForkNow } from '@/features/thread/forkNow';
import type { Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

type End = Pick<Job, 'state' | 'error'>;
const calls = vi.hoisted(() => ({ fork: vi.fn<typeof Forks.forkNow>(), wait: vi.fn<(...args: unknown[]) => Promise<End>>(),
  navigate: vi.fn<(_destination: unknown) => Promise<void>>(), toast: vi.fn<(_notice: unknown) => void>(), change: vi.fn<(...args: unknown[]) => void>() }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), forkNow: calls.fork }));
vi.mock('@/features/change/target', () => ({ openChange: calls.change }));
vi.mock('@/realtime/queryFeeds', async (original) => ({ ...await original<object>(), waitForJob: calls.wait }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => calls.navigate }));
vi.mock('@/ui/Toast', async (original) => ({ ...await original<object>(), toast: calls.toast }));
function Fork() {
  const fork = useForkNow('source');
  return <button type="button" disabled={fork.pending} onClick={() => fork.start(1)}>Fork</button>;
}
const previous = client.session.getState();
beforeEach(() => {
  client.session.setState({ status: 'signed_in', userId: 'fork-reader', readerVersion: 970 });
  calls.wait.mockResolvedValue({ state: 'succeeded', error: null });
  vi.clearAllMocks(); calls.navigate.mockResolvedValue(undefined);
  calls.fork.mockResolvedValue({ thread_id: 'made-thread', job: {
    id: 'fork-job', kind: 'thread_fork', node_id: 'source', environment_id: null, provider_id: null, batch_id: null,
    state: 'queued', result: null, error: null, log: null, created_by: null,
    created_at: '2026-10-15T17:40:00Z', started_at: null, finished_at: null,
    progress: { phase: 'lock', phases: [], percent: 0, files_done: 0, files_total: null, bytes_done: 0,
      bytes_total: null, rate_bps: null, eta_seconds: null, message: null },
  } });
});
afterEach(async () => { cleanup(); client.session.setState(previous); await setLocale('en'); });

it('opens the completed fork after its native/model-input commit, without a refresh or a second click', async () => {
  let finish: ((end: End) => void) | undefined;
  calls.wait.mockImplementation(() => new Promise<End>((resolve) => { finish = resolve; }));
  show(<Fork />);
  await userEvent.click(screen.getByRole('button', { name: 'Fork' }));
  await waitFor(() => expect(calls.wait).toHaveBeenCalledTimes(1));
  expect(calls.navigate).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Fork' })).toBeDisabled();
  await act(async () => { finish?.({ state: 'succeeded', error: null }); await Promise.resolve(); });
  await waitFor(() => expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/thread/$threadId', params: { threadId: 'made-thread' } }));
  expect(calls.fork).toHaveBeenCalledExactlyOnceWith('source', expect.any(String), 1);
  expect(calls.toast).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('leaves the source open and gives a readable retry reason for a failed save (%s)', async (locale) => {
  await setLocale(locale);
  calls.wait.mockResolvedValue({ state: 'failed', error: { code: 'internal', phase: 'commit', undone: true, message: 'model_inputs_segment_id_fkey' } });
  show(<Fork />);
  await userEvent.click(screen.getByRole('button', { name: 'Fork' }));
  await waitFor(() => expect(calls.toast).toHaveBeenCalledWith({ tone: 'danger', title: i18n.t('change.forkNow.failed'), description: i18n.t('errors.internal') }));
  expect(calls.navigate).not.toHaveBeenCalled();
  expect(JSON.stringify(calls.toast.mock.calls)).not.toContain('model_inputs_segment_id_fkey');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Fork' })).toBeEnabled());
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps a sparse accepted fork separate from a failed write (${locale})`, async () => {
  await setLocale(locale); calls.fork.mockResolvedValue({ projection: 'receipt', job_id: 'accepted-job', thread_id: 'reserved-thread' });
  show(<Fork />); await userEvent.click(screen.getByRole('button', { name: 'Fork' }));
  await waitFor(() => expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'source' }, { kind: 'fork', job: 'accepted-job' }));
  expect(calls.wait).not.toHaveBeenCalled(); expect(calls.navigate).not.toHaveBeenCalled();
  expect(calls.toast).toHaveBeenCalledWith({ tone: 'info', title: i18n.t('duplicate.forkStarted') });
});
it('a progress reader failure after acceptance reopens that actual job without another copy request', async () => {
  calls.wait.mockRejectedValue(new NetworkError('failed')); show(<Fork />); await userEvent.click(screen.getByRole('button', { name: 'Fork' }));
  await waitFor(() => expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'source' }, { kind: 'fork', job: 'fork-job' }));
  expect(calls.fork).toHaveBeenCalledOnce(); expect(calls.toast).toHaveBeenCalledWith({ tone: 'info', title: i18n.t('duplicate.forkStarted') });
});
it('a lost native fork answer retries the same act key, then a completed copy permits a new deliberate copy', async () => {
  calls.fork.mockRejectedValueOnce(new NetworkError('failed')); show(<Fork />); const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Fork' })); await waitFor(() => expect(calls.toast).toHaveBeenCalled());
  await user.click(screen.getByRole('button', { name: 'Fork' })); await waitFor(() => expect(calls.navigate).toHaveBeenCalledOnce());
  expect(calls.fork.mock.calls[0]?.[1]).toBe(calls.fork.mock.calls[1]?.[1]);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Fork' })).toBeEnabled()); await user.click(screen.getByRole('button', { name: 'Fork' }));
  await waitFor(() => expect(calls.fork).toHaveBeenCalledTimes(3)); expect(calls.fork.mock.calls[2]?.[1]).not.toBe(calls.fork.mock.calls[1]?.[1]);
});
it('a late successful fork under a retired reader never takes the replacement reader to it', async () => {
  let finish: ((end: End) => void) | undefined;
  calls.wait.mockImplementation(() => new Promise((resolve) => { finish = resolve; })); show(<Fork />);
  await userEvent.click(screen.getByRole('button', { name: 'Fork' })); await waitFor(() => expect(calls.wait).toHaveBeenCalledOnce());
  act(() => client.session.setState({ readerVersion: 971 }));
  await act(async () => { finish?.({ state: 'succeeded', error: null }); await Promise.resolve(); });
  expect(calls.navigate).not.toHaveBeenCalled(); expect(calls.change).not.toHaveBeenCalled(); expect(calls.toast).not.toHaveBeenCalled();
});
