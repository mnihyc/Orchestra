/** Restoring a directory to before a turn: the plan is read before anything is written, a stale plan is refused and planned again with the choices kept, the job is asked for once, and its undo plans the way back. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { RestoreDialog } from '@/features/source/Restore';
import type { Job, RestorePlan, Snapshot, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ plans: [] as { snapshot: string; options: unknown }[], confirmed: [] as { preflight: string; key: string }[], job: null as Job | null, stale: false, snapshotReason: null as string | null }));

const environment = { id: 'env-1', name: 'Main', owner: { id: 'u1', username: 'ada', display_name: 'Ada Lovelace' } } as unknown as Thread['environment'];

function snapshot(id: string): Snapshot {
  return {
    completed_at: '2026-10-14T10:00:00.000Z', created_at: '2026-10-14T10:00:00.000Z', created_by: null, directory: 'web', env_generation: 1, environment: environment!,
    expires_at: null, file_count: 3, id, kind: 'turn', label: '', missing_count: 0, repository_head: null, selection: { include: [], exclude: [] }, state: calls.snapshotReason === null ? 'complete' : 'unavailable', storage: 'environment',
    thread_id: 't1', total_bytes: 30, unavailable_reason: calls.snapshotReason,
  };
}

function planFor(snapshotId: string, count: number): RestorePlan {
  const paths: RestorePlan['paths'] = [
    { action: 'change', expected: 'h1', later_change: true, later_writers: [{ id: 't2', kind: 'thread', name: 'Fix login', redacted: false }], path: 'src/login.ts' },
    { action: 'add', expected: 'none', later_change: null, later_writers: [], path: 'src/old.ts' },
  ];
  return {
    blockers: [], directory: 'web', environment: environment!, expires_at: '2026-10-14T10:15:00.000Z', generation: 1, ignore_fingerprint: 'i', ignored: ['node_modules'],
    namespace_fingerprint: 'n', options: { active_turn: 'wait', keep_previous: false, stop_processes: false, verify_timeout_secs: 60, wait_minutes: 10 },
    paths: paths.slice(0, count),
    preflight_id: `plan-${snapshotId}-${calls.plans.length}`, snapshot: snapshot(snapshotId), undo_available: true,
    writers: [{ thread: { id: 't2', kind: 'thread', name: 'Fix login', redacted: false }, active: true }],
  };
}

vi.mock('@/api/source', () => ({
  snapshotQuery: (id: string) => ({ queryKey: ['test-snapshot', id], queryFn: () => Promise.resolve(snapshot(id)) }),
  planRestore: (_environment: string, snapshotId: string, options: unknown) => {
    calls.plans.push({ snapshot: snapshotId, options });
    return Promise.resolve(planFor(snapshotId, 2));
  },
  restoreFiles: (_environment: string, preflight: string, key: string) => {
    calls.confirmed.push({ preflight, key });
    if (calls.stale) {
      calls.stale = false;
      return Promise.reject(new ApiError({ type: '', code: 'workspace.restore_conflict', title: 'changed', status: 409, detail: 'changed', request_id: 'r' }, null));
    }
    return Promise.resolve({ id: 'job-1' } as Job);
  },
}));

vi.mock('@/api/forks', () => ({
  isOver: (state: string) => ['succeeded', 'failed', 'cancelled'].includes(state),
  jobQuery: (id: string) => ({ queryKey: ['test-job', id], queryFn: () => Promise.resolve(calls.job) }),
}));

const thread = { id: 't1', name: 'Fix login', directory: 'web', environment, environment_permissions: ['files.view', 'files.manage'] } as unknown as Thread;

function draw(onClose = vi.fn()) {
  show(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RestoreDialog thread={thread} snapshotId="before-7" onClose={onClose} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  calls.snapshotReason = null;
  calls.plans = [];
  calls.confirmed = [];
  calls.stale = false;
  calls.job = { id: 'job-1', state: 'succeeded', result: { snapshot_id: 'before-7', undo_snapshot_id: 'undo-9' }, progress: { percent: 100, phase: 'done', phases: [] }, error: null } as unknown as Job;
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it.each([
    ['store_unavailable', 'source.restore.why.storeUnavailable'],
    ['future_recording_failure', 'source.restore.why.other'],
  ] as const)(`explains %s in ${locale}/${theme} without exposing a protocol code`, async (reason, key) => {
    const previousTheme = document.documentElement.getAttribute('data-theme');
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    calls.snapshotReason = reason;
    try {
      draw();
      expect(await screen.findByText(i18n.t(key))).toBeVisible();
      expect(screen.queryByText(new RegExp(reason))).not.toBeInTheDocument();
      expect(calls.confirmed).toHaveLength(0);
    } finally {
      await setLocale('en');
      if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.dataset.theme = previousTheme;
    }
  });
}

describe('a restore is planned before anything is written', () => {
  it('shows the paths, the later change with who made it, the writer at work and the ignored paths before confirming', async () => {
    draw();
    expect(await screen.findByText('src/login.ts')).toBeInTheDocument();
    expect(screen.getByText('Changed after the turn by Fix login.')).toBeInTheDocument();
    expect(screen.getByText('Whether it changed after the turn is not known.')).toBeInTheDocument();
    expect(screen.getByText('Fix login is writing in this directory now.')).toBeInTheDocument();
    expect(screen.getByText('1 ignored path is left as it is')).toBeInTheDocument();
    expect(calls.confirmed).toEqual([]);
    expect(calls.plans).toHaveLength(1);
  });

  it('keeps the choice made when the plan is asked again for it', async () => {
    const user = userEvent.setup();
    draw();
    await screen.findByText('src/login.ts');
    await user.click(screen.getByRole('radio', { name: 'Interrupt' }));
    await waitFor(() => expect(calls.plans).toHaveLength(2));
    expect(calls.plans[1]?.options).toEqual({ active_turn: 'interrupt' });
    await screen.findByText('src/login.ts');
    expect(screen.getByRole('radio', { name: 'Interrupt' })).toBeChecked();
    expect(calls.confirmed).toEqual([]);
  });

  it('refuses a stale plan without writing, then confirms a fresh one with a new act', async () => {
    const user = userEvent.setup();
    calls.stale = true;
    draw();
    await user.click(await screen.findByRole('button', { name: 'Restore these files' }));
    expect(await screen.findByText(/Nothing was written. Make a fresh plan/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Make a fresh plan' }));
    await waitFor(() => expect(calls.plans).toHaveLength(2));
    await user.click(await screen.findByRole('button', { name: 'Restore these files' }));
    await waitFor(() => expect(calls.confirmed).toHaveLength(2));
    expect(calls.confirmed[0]?.preflight).not.toBe(calls.confirmed[1]?.preflight);
    expect(calls.confirmed[0]?.key).not.toBe(calls.confirmed[1]?.key);
  });

  it('follows the one job and plans the undo from the state it kept', async () => {
    const user = userEvent.setup();
    draw();
    await user.click(await screen.findByRole('button', { name: 'Restore these files' }));
    expect(await screen.findByText('The files were restored.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Undo this restore…' }));
    await waitFor(() => expect(calls.plans.at(-1)?.snapshot).toBe('undo-9'));
    expect(await screen.findByRole('button', { name: 'Undo the restore' })).toBeInTheDocument();
    expect(calls.confirmed).toHaveLength(1);
  });
});
