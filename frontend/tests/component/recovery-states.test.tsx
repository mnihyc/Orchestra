/** Recovery rows use the same named status component for waiting, active and every terminal outcome. */
import { screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MoveDialog } from '@/features/fleet/MoveDialog';
import type { EnvironmentBatch, Job } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const fixtures = vi.hoisted(() => ({ batch: null as unknown as EnvironmentBatch, jobs: new Map<string, Job>() }));
vi.mock('@/api/loss', async (original) => ({ ...await original<object>(), recoveryBatchQuery: () => ({ queryKey: ['status-batch'], queryFn: () => Promise.resolve(fixtures.batch) }) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), jobQuery: (id: string) => ({ queryKey: ['status-job', id], queryFn: () => Promise.resolve(fixtures.jobs.get(id)) }) }));

beforeEach(() => {
  const states = ['queued', 'running', 'succeeded', 'failed', 'cancelled', 'lost'] as const;
  fixtures.jobs.clear();
  fixtures.batch = { batch_id: 'status-batch', jobs: states.map((state) => {
    const job: Job = { id: `job-${state}`, batch_id: 'status-batch', created_by: null, environment_id: `environment-${state}`, node_id: null, provider_id: null, log: null, result: null, kind: 'environment_provision', state, error: null, created_at: '2026-10-25T00:00:00Z', started_at: '2026-10-25T00:00:00Z', finished_at: null, progress: { message: null, phases: [], files_done: 2, files_total: null, bytes_done: 1024, bytes_total: null, rate_bps: null, eta_seconds: null, phase: 'files', percent: state === 'queued' ? 0 : state === 'running' ? 50 : 100 } };
    fixtures.jobs.set(job.id, job);
    return { environment: { id: `environment-${state}`, name: `Environment ${state}`, status: state === 'succeeded' ? 'running' : 'lost' }, job };
  }) } as EnvironmentBatch;
});

it('keeps independent queued, active, successful, failed, cancelled and lost rows visible with named states', async () => {
  show(<MoveDialog source={{ ids: [] }} batch="status-batch" open onClose={vi.fn()} />);
  for (const state of ['queued', 'running', 'succeeded', 'failed', 'cancelled', 'lost'] as const) {
    if (state === 'running' || state === 'queued') {
      const row = (await screen.findByRole('link', { name: `Environment ${state}` })).closest('tr');
      expect(row?.querySelector('[data-job-activity]')).toBeVisible();
      expect(row?.querySelector('[data-job-elapsed]')).toBeVisible();
      if (state === 'running') expect(row?.querySelector('[data-job-line]')).toHaveTextContent(i18n.t('liveWork.copying'));
    } else {
      const word = await screen.findByText(i18n.t(`source.job.${state}`));
      expect(word.closest('[data-status-chip]')).not.toBeNull();
    }
    expect(screen.getByRole('link', { name: `Environment ${state}` })).toBeVisible();
  }
  expect(screen.queryByText('files')).toBeNull();
  expect(screen.getByRole('progressbar')).toHaveAccessibleName(i18n.t('loss.progressRunning'));
});
