/** Cancelling an import stops its job and opens the partial report when the server returns it. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ImportProgress } from '@/features/people/ImportProgress';
import type { Job } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn(), cancel: vi.fn(), lookup: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(),
  Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a>,
}));
vi.mock('@/api/exports', async (original) => ({ ...await original<object>(),
  jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: () => calls.read(id) as Promise<unknown>, retry: false }), cancelJob: calls.cancel,
}));
vi.mock('@/api/imports', async (original) => ({ ...await original<object>(), importForJob: calls.lookup }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const job: Job = { id: 'job', kind: 'import', state: 'running', result: null,
  progress: { phase: 'create', percent: 42, bytes_done: 100, bytes_total: 200, files_done: 1, files_total: 2, eta_seconds: null, rate_bps: null, message: null, phases: [] },
  batch_id: null, created_by: null, node_id: 'place', environment_id: null, provider_id: null, error: null, log: null,
  created_at: '2026-10-28T12:00:00Z', started_at: '2026-10-28T12:00:01Z', finished_at: null };

beforeEach(() => { calls.read.mockReset(); calls.cancel.mockReset(); calls.lookup.mockReset(); calls.read.mockResolvedValue(job); });

it('shows the measured phase and opens a report after cancellation', async () => {
  calls.cancel.mockResolvedValue({ ...job, state: 'cancelled', result: { import_id: 'report' }, finished_at: '2026-10-28T12:00:02Z' });
  const user = userEvent.setup(), open = vi.fn();
  show(<ImportProgress id="job" onReport={open} />);
  expect(await screen.findByText(/Creating · 1 \/ 2 items · 100 B/)).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Cancel import' }));
  await waitFor(() => expect(open).toHaveBeenCalledExactlyOnceWith('report'));
  expect(calls.cancel).toHaveBeenCalledExactlyOnceWith('job');
});

it('says when a lost job cannot yet find its report', async () => {
  calls.read.mockResolvedValue({ ...job, state: 'lost', finished_at: '2026-10-28T12:00:02Z' });
  calls.lookup.mockResolvedValue(null);
  show(<ImportProgress id="job" onReport={vi.fn()} />);
  expect(await screen.findByText('Import failed')).toBeVisible();
  expect(await screen.findByText('Find the report in Your imports')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Your imports' })).toHaveAttribute('href', '/settings/imports');
});
