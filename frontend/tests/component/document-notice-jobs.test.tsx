/** Background discussion delivery uses the common progress line, bounded pages and cancellation. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClientProvider } from '@tanstack/react-query';
import type * as Exports from '@/api/exports';
import type * as Queries from '@/api/documentNoticeJobs';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { LiveQueryClient } from '@/api/query';
import { DocumentNoticeJobs } from '@/features/people/DocumentNoticeJobs';
import type { Job } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ stop: vi.fn<typeof Exports.cancelJob>(), pages: vi.fn<(cursor: string | null) => Promise<{ items: Job[]; next_cursor: string | null }>>() }));
vi.mock('@/api/exports', async (original) => ({ ...await original<typeof Exports>(), cancelJob: calls.stop }));
vi.mock('@/api/documentNoticeJobs', async (original) => ({ ...await original<typeof Queries>(), documentNoticeJobsQuery: (id: string, reader = 0) => ({ queryKey: ['document-notice-jobs', id, reader], initialPageParam: null as string | null, maxPages: 1, queryFn: ({ pageParam }: { pageParam: string | null }) => calls.pages(pageParam), getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor ?? undefined }) }));
const priorSession = client.session.getState();
afterEach(() => client.session.setState(priorSession));
const job: Job = { id: 'job-0', kind: 'document_notices', node_id: null, environment_id: null, provider_id: null, batch_id: null, created_by: null,
  state: 'running', result: null, error: null, log: null, call_recovery: null, created_at: '2026-10-15T16:00:00Z', started_at: '2026-10-15T16:00:00Z', finished_at: null,
  progress: { phase: 'notify_people', percent: 0, phases: [{ name: 'notify_people', state: 'running', weight: 100 }], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: '200' } };

it('shows checked people, pages without growing rows and stops a chosen job', async () => {
  const reader = (priorSession.readerVersion ?? 0) + 1;
  client.session.setState({ status: 'signed_in', userId: 'person', readerVersion: reader });
  const hidden = new Set<string>();
  calls.pages.mockImplementation((cursor) => { const offset = Number(cursor ?? 0); return Promise.resolve({ items: Array.from({ length: 5 }, (_, index) => ({ ...job, id: `job-${offset + index}` })).filter((row) => !hidden.has(row.id)), next_cursor: offset < 10 ? String(offset + 5) : null }); });
  calls.stop.mockImplementation((id) => { hidden.add(id); return Promise.resolve({ ...job, id, state: 'cancelled' }); });
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  const retained = { pages: [{ items: [job], next_cursor: null }], pageParams: [null] };
  for (let index = 1; index <= 1000; index += 1) cache.setQueryData(['document-notice-jobs', 'doc', reader + index], retained);
  const input = userEvent.setup(); const view = show(<QueryClientProvider client={cache}><DocumentNoticeJobs document="doc" /></QueryClientProvider>);
  try {
  await screen.findAllByRole('button', { name: 'Stop notifications' });
  expect(view.container.querySelectorAll('[data-notice-job]')).toHaveLength(5);
  expect(view.container).toHaveTextContent('Notifying people · 200 checked');
  await input.click(screen.getByRole('button', { name: 'Show more' }));
  await waitFor(() => expect(view.container.querySelector('[data-notice-job="job-5"]')).toBeInTheDocument());
  expect(view.container.querySelector('[data-notice-job="job-0"]')).toBeNull();
  expect(view.container.querySelectorAll('[data-notice-job]')).toHaveLength(5);
  await input.click(screen.getByRole('button', { name: 'Latest notifications' }));
  await waitFor(() => expect(view.container.querySelector('[data-notice-job="job-0"]')).toBeInTheDocument());
  const beforeStop = calls.pages.mock.calls.length;
  await input.click(screen.getAllByRole('button', { name: 'Stop notifications' })[0] ?? screen.getByRole('button', { name: 'Stop notifications' }));
  await waitFor(() => expect(calls.stop).toHaveBeenCalledWith('job-0'));
  await waitFor(() => expect(view.container.querySelector('[data-notice-job="job-0"]')).toBeNull());
  expect(calls.pages).toHaveBeenCalledTimes(beforeStop + 1);
  const others = cache.getQueryCache().findAll({ queryKey: ['document-notice-jobs', 'doc'] }).filter((query) => query.queryKey[2] !== reader);
  expect(others).toHaveLength(1000);
  expect(others.every((query) => query.state.data === retained)).toBe(true);
  } finally { view.unmount(); cache.clear(); }
});
