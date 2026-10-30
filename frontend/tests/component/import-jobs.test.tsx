/** Import notices are opt-in and a watched job survives closing its dialog. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { useImportJobWatch, useWatchedImports, watchImport } from '@/features/people/importJobs';
import { ImportStatus } from '@/features/people/ImportStatus';
import type { Job } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn(), toast: vi.fn(), open: vi.fn(), navigate: vi.fn(), node: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: () => calls.node(id) as Promise<unknown> }) }));
vi.mock('@/api/exports', async (original) => ({ ...await original<object>(), jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: () => calls.read(id) as Promise<unknown>, retry: false }) }));
vi.mock('@/ui/Toast', async (original) => ({ ...await original<object>(), toast: calls.toast }));

const progress: Job['progress'] = { phase: 'create', percent: 42, bytes_done: 100, bytes_total: 200, files_done: 1, files_total: 2, eta_seconds: null, rate_bps: null, message: null, phases: [] };
const job = (id: string, state: Job['state']): Job => ({ id, kind: 'import', state, progress, result: state === 'succeeded' ? { import_id: `report-${id}` } : null,
  batch_id: null, created_by: null, node_id: 'place', environment_id: null, provider_id: null, error: null, log: null,
  created_at: '2026-10-28T12:00:00Z', started_at: '2026-10-28T12:00:01Z', finished_at: state === 'succeeded' ? '2026-10-28T12:00:02Z' : null });
const before = client.session.getState();
function Watch() { useImportJobWatch(calls.open); return null; }

beforeEach(() => {
  calls.read.mockReset(); calls.toast.mockReset(); calls.open.mockReset(); calls.navigate.mockReset(); calls.node.mockReset(); sessionStorage.clear();
  useWatchedImports.setState({ user: null, items: [] });
  client.session.setState({ ...before, userId: 'person', status: 'signed_in' });
  calls.read.mockImplementation((id: string) => Promise.resolve(job(id, 'succeeded')));
});
afterEach(() => { useWatchedImports.setState({ user: null, items: [] }); sessionStorage.clear(); client.session.setState(before); });

it('updates a closed-dialog job and sends a notice only when chosen', async () => {
  show(<Watch />);
  await waitFor(() => expect(useWatchedImports.getState().user).toBe('person'));
  act(() => watchImport(job('quiet', 'running'), 'place', false));
  await waitFor(() => expect(useWatchedImports.getState().items.find((item) => item.job === 'quiet')?.state).toBe('done'));
  expect(calls.toast).not.toHaveBeenCalled();
  act(() => watchImport(job('notice', 'running'), 'place', true));
  await waitFor(() => expect(calls.toast).toHaveBeenCalledOnce());
  act(() => watchImport(job('notice', 'running'), 'place', true));
  expect(useWatchedImports.getState().items.find((item) => item.job === 'notice')?.state).toBe('done');
  expect(calls.toast).toHaveBeenCalledOnce();
  const notice = calls.toast.mock.calls[0]?.[0] as { action: { run: () => void } };
  act(() => notice.action.run());
  expect(calls.open).toHaveBeenCalledWith(expect.objectContaining({ job: 'notice', report: 'report-notice' }));
});

it('does not show another account’s watched job while the session changes', () => {
  useWatchedImports.setState({ user: 'previous-person', items: [{ job: 'private-job', place: 'place', report: null, undo: false, notify: false, state: 'running', percent: 42, reconnecting: false }] });
  show(<ImportStatus />);
  expect(screen.queryByText(/42%/)).not.toBeInTheDocument();
});

it('resumes watching the same person’s running job after a tab reload', async () => {
  calls.read.mockImplementation((id: string) => Promise.resolve(job(id, 'running')));
  const view = show(<Watch />);
  await waitFor(() => expect(useWatchedImports.getState().user).toBe('person'));
  act(() => watchImport(job('running', 'running'), 'place', false));
  view.unmount();
  useWatchedImports.setState({ user: null, items: [] });
  show(<Watch />);
  await waitFor(() => expect(useWatchedImports.getState().items).toEqual([expect.objectContaining({ job: 'running', state: 'running' })]));
  expect(calls.toast).not.toHaveBeenCalled();
});

it('draws one history link for 100,000 watched imports without requesting their places', () => {
  useWatchedImports.setState({ user: 'person', items: Array.from({ length: 100_000 }, (_, index) => ({
    job: `job-${index}`, place: `place-${index}`, report: `report-${index}`, undo: false, notify: false,
    state: 'done' as const, percent: 100, reconnecting: false,
  })) });
  show(<ImportStatus />);
  expect(screen.getAllByRole('button')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: '100,000 imports' }));
  expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/settings/imports' });
  expect(calls.node).not.toHaveBeenCalled();
  expect(calls.read).not.toHaveBeenCalled();
});
