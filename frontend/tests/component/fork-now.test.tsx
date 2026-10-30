/** R1 W09: the source remains open until a successful live result, then its fork opens exactly once. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Forks from '@/api/forks';
import { useForkNow } from '@/features/thread/forkNow';
import type { Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

type End = Pick<Job, 'state' | 'error'>;
const calls = vi.hoisted(() => ({ fork: vi.fn<typeof Forks.forkNow>(), wait: vi.fn<(...args: unknown[]) => Promise<End>>(),
  navigate: vi.fn<(_destination: unknown) => Promise<void>>(), toast: vi.fn<(_notice: unknown) => void>() }));
vi.mock('@/api/forks', () => ({ forkNow: calls.fork }));
vi.mock('@/realtime/live', () => ({ socket: {} }));
vi.mock('@/realtime/queryFeeds', () => ({ waitForJob: calls.wait }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => calls.navigate }));
vi.mock('@/ui/Toast', async (original) => ({ ...await original<object>(), toast: calls.toast }));
function Fork() {
  const fork = useForkNow('source');
  return <button type="button" disabled={fork.pending} onClick={() => fork.start(1)}>Fork</button>;
}
beforeEach(() => {
  vi.clearAllMocks(); calls.navigate.mockResolvedValue(undefined);
  calls.fork.mockResolvedValue({ thread_id: 'made-thread', job: {
    id: 'fork-job', kind: 'thread_fork', node_id: 'source', environment_id: null, provider_id: null, batch_id: null,
    state: 'queued', result: null, error: null, log: null, created_by: null,
    created_at: '2026-10-15T17:40:00Z', started_at: null, finished_at: null,
    progress: { phase: 'lock', phases: [], percent: 0, files_done: 0, files_total: null, bytes_done: 0,
      bytes_total: null, rate_bps: null, eta_seconds: null, message: null },
  } });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

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
