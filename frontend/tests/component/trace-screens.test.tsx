/** QS-003 and QS-013: real inspection envelopes and served host facts remain readable without invented requests or acts. */
import { QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { LiveQueryClient } from '@/api/query';
import { InspectionBanner, InspectionPage } from '@/features/trace/Inspection';
import { KeptEnvironmentRow, KeptEnvironments, RemovalFacts } from '@/features/trace/KeptEnvironments';
import { HostKept } from '@/features/trace/HostKept';
import type { Host, HostKeptData, Job, ThreadSync, DeletedEnvironment, DeletedInspection as Inspection } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const read = vi.hoisted(() => vi.fn<(path: string, options?: unknown) => Promise<unknown>>());
const retention = vi.hoisted(() => ({ list: vi.fn(), restore: vi.fn(), job: vi.fn() }));
vi.mock('@/api/trace', async (original) => ({ ...await original<object>(), traceJson: read,
  retainedEnvironmentQuery: (id: string) => ({ queryKey: ['inspection-test', id], queryFn: () => read(`/environments/${id}?inspect_deleted=true`) }),
  keptEnvironmentsQuery: (id: string) => ({ queryKey: ['trace-deleted', 'environments', id], queryFn: retention.list, initialPageParam: null, getNextPageParam: () => null }),
  restoreEnvironment: retention.restore,
  retentionJobQuery: (id: string) => ({ queryKey: ['retention-job-test', id], queryFn: retention.job }),
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: vi.fn() }));
vi.mock('@/features/timeline/Timeline', () => ({ StoredTimeline: ({ snapshot }: { snapshot: Pick<ThreadSync, 'thread'> }) => <div data-stored-timeline>{snapshot.thread.name}</div> }));

const person = userRef(me(9, 'dora', 'Dora Developer'));
const inspection: Inspection = { deleted_at: '2026-10-12T12:00:00Z', deleted_by: person, purge_at: '2026-10-19T12:00:00Z', readonly: true };
const environment = { id: 'env', name: 'Kept workspace', address: 'team.cache', owner: person, placement: 'managed' as const, status: 'removed' as const, ports_exposed: false };
const home = { id: 'home', kind: 'project' as const, name: 'Web', redacted: false };
const row: Omit<DeletedEnvironment, 'removal'> = { environment, home, host: null, storage: null };
const threadReply = (facts: Inspection | null | undefined) => ({ thread: { id: 'thread', name: 'Archived conversation', path: [], ...(facts === undefined ? {} : { inspection: facts }) }, items: [], turns: [], inputs: [], segments: [], has_older: false });

beforeEach(() => { read.mockReset(); retention.list.mockReset(); retention.restore.mockReset(); retention.job.mockReset(); });

it('QS-003 reads a thread banner from thread.inspection with no top-level copy', async () => {
  read.mockResolvedValue(threadReply(inspection));
  show(<InspectionPage kind="thread" id="thread" />);
  expect(await screen.findByText(/Deleted by Dora Developer/)).toBeInTheDocument();
  expect(screen.getByText('Read-only inspection. Reading changes no read marks or notifications.')).toBeInTheDocument();
  expect(screen.getByText('Archived conversation')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
  expect(read).toHaveBeenCalledWith('/threads/thread/sync?inspect_deleted=true&limit=200', expect.anything());
});

it.each([null, undefined])('QS-003 absent thread inspection metadata (%s) offers no retained content or acts', async (facts) => {
  read.mockResolvedValue(threadReply(facts));
  show(<InspectionPage kind="thread" id="thread" />);
  expect(await screen.findByText('No content remains to inspect.')).toBeInTheDocument();
  expect(screen.queryByText('Archived conversation')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
});

it('QS-003 document inspection uses its actual document.inspection member', async () => {
  read.mockResolvedValue({ document: { id: 'doc', title: 'Retained notes', path: [], inspection }, blocks: [], discussions: [] });
  show(<InspectionPage kind="document" id="doc" />);
  expect(await screen.findByRole('heading', { name: 'Retained notes' })).toBeInTheDocument();
  expect(screen.getByText(/Deleted by Dora Developer/)).toBeInTheDocument();
  expect(screen.getByText('Read-only inspection. Reading changes no read marks or notifications.')).toBeInTheDocument();
});

it.each([null, undefined])('QS-003 absent environment removal (%s) is empty and cannot restore or purge', async (removal) => {
  read.mockResolvedValue({ ...environment, home, inspection, ...(removal === undefined ? {} : { removal }) });
  show(<InspectionPage kind="environment" id="env" />);
  expect(await screen.findByText('No content remains to inspect.')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Bring it back' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Remove for good' })).not.toBeInTheDocument();
});

it.each([null, undefined])('QS-003 a partial kept row (%s) is guarded before its hooks or actions mount', (removal) => {
  show(<KeptEnvironmentRow entry={{ ...row, removal }} />);
  expect(screen.getByText('No content remains to inspect.')).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('QS-003 a missing banner or removal record never dereferences deletion facts', () => {
  show(<><InspectionBanner /><RemovalFacts entry={row} /></>);
  expect(screen.getByText('No content remains to inspect.')).toBeInTheDocument();
});

it('rereads the visible kept list after a competing purge refuses a stale restore', async () => {
  const removal: DeletedEnvironment['removal'] = {
    phase: 'kept', deleted_at: inspection.deleted_at, deleted_by: person, purge_at: inspection.purge_at,
    keep_days: 7, purged_at: null, bytes: 0, size_at: null, reason: null, revision: 1,
    can_inspect: true, can_restore: true, can_purge: true, job: null,
  };
  retention.list.mockResolvedValueOnce({ items: [{ ...row, removal }], next_cursor: null })
    .mockResolvedValue({ items: [], next_cursor: null });
  retention.restore.mockRejectedValue(new ApiError({ type: 'orch:error:resource.in_use', code: 'resource.in_use',
    status: 409, title: 'A purge holds the files', detail: 'A purge holds the files', request_id: 'purge-race' }, null));
  const client = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = show(<QueryClientProvider client={client}><KeptEnvironments node={home.id} open /></QueryClientProvider>);
  try {
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('trace.restore') }));
    expect(await screen.findByText(i18n.t('trace.keptEmpty'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('trace.restore') })).not.toBeInTheDocument();
    expect(retention.list).toHaveBeenCalledTimes(2);
  } finally {
    view.unmount();
    client.clear();
  }
});

it('rereads the owning kept list once after a failed restore and keeps its original deadline', async () => {
  const removal: DeletedEnvironment['removal'] = {
    phase: 'kept', deleted_at: inspection.deleted_at, deleted_by: person, purge_at: inspection.purge_at,
    keep_days: 7, purged_at: null, bytes: 0, size_at: null, reason: null, revision: 1,
    can_inspect: true, can_restore: true, can_purge: true, job: null,
  };
  const job: Job = { id: 'restore-job', kind: 'environment_restore', state: 'queued', environment_id: environment.id,
    node_id: home.id, provider_id: null, created_at: inspection.deleted_at, created_by: person,
    started_at: null, finished_at: null, error: null, log: null, result: null, batch_id: null,
    progress: { bytes_done: 0, bytes_total: 0, eta_seconds: null, files_done: 0, files_total: null,
      message: null, rate_bps: null, phases: [], percent: 0, phase: 'Waiting for kept files' } };
  const reason = 'The host has no free disk space; the original kept volumes remain.';
  retention.list.mockResolvedValueOnce({ items: [{ ...row, removal }], next_cursor: null })
    .mockResolvedValueOnce({ items: [{ ...row, removal: { ...removal, phase: 'restoring', can_restore: false } }], next_cursor: null })
    .mockResolvedValue({ items: [{ ...row, removal: { ...removal, reason, revision: 3 } }], next_cursor: null });
  retention.restore.mockResolvedValue(job);
  retention.job.mockResolvedValue(job);
  const client = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = show(<QueryClientProvider client={client}><KeptEnvironments node={home.id} open /></QueryClientProvider>);
  try {
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('trace.restore') }));
    await waitFor(() => expect(retention.list).toHaveBeenCalledTimes(2));
    act(() => { client.setQueryData(['retention-job-test', job.id], { ...job, state: 'failed',
      error: { code: 'environment.unavailable', message: reason, phase: 'Checking kept files', undone: true } }); });
    expect(await screen.findByText(reason, { exact: true })).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('trace.restore') })).toBeEnabled();
    expect(screen.getByText(/^Kept until /)).toHaveTextContent('Oct 19, 2026');
    expect(retention.list).toHaveBeenCalledTimes(3);
  } finally {
    view.unmount();
    client.clear();
  }
});

const kept: HostKeptData[] = [
  { environment_id: 'visible-env', name: 'Empty kept workspace', home, storages: [null, 'cold'], phase: 'kept', bytes: 0, purge_at: inspection.purge_at, job: null },
  { environment_id: 'private-env', name: null, home: { ...home, name: 'hidden-home', redacted: true }, storages: ['ssd'], phase: 'purging', bytes: null, purge_at: null,
    job: { id: 'failed-job', kind: 'environment_purge', state: 'failed', phase: 'volumes', percent: 50 } },
];
const host = (permissions: string[] = ['host.manage']): Pick<Host, 'kept_data' | 'access'> => ({ kept_data: kept, access: { permissions, administers: false, through: [], admin_only: [], participating: true } });

it('QS-013 renders the host payload with known zero, unknown size, redaction, storages and a safe failed job', () => {
  show(<HostKept host={host()} />);
  const section = screen.getByRole('heading', { name: 'Kept after removal' }).closest('section');
  expect(section).not.toBeNull();
  expect(screen.getByText('Empty kept workspace')).toBeInTheDocument();
  expect(screen.getByText('0 B')).toBeInTheDocument();
  expect(screen.getByText('Default, cold')).toBeInTheDocument();
  expect(screen.getByText('Private environment')).toBeInTheDocument();
  expect(screen.queryByText('hidden-home')).not.toBeInTheDocument();
  const privateRow = screen.getByText('Private environment').closest('li');
  if (privateRow === null) throw new Error('The private environment needs its kept-data row');
  expect(within(privateRow).getByText('Failed', { exact: true })).toBeVisible();
  expect(within(privateRow).queryByText(/failed-job|environment_purge|volumes/u)).not.toBeInTheDocument();
  const sizeValue = within(privateRow).getByText(i18n.t('trace.size'), { selector: 'dt' }).nextElementSibling;
  const homeValue = within(privateRow).getByText(i18n.t('trace.home'), { selector: 'dt' }).nextElementSibling;
  expect(sizeValue).toHaveTextContent(i18n.t('trace.unknown'));
  expect(homeValue).toHaveTextContent(i18n.t('trace.unknown'));
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(read).not.toHaveBeenCalled();
});

it('QS-013 a host view grant alone exposes no kept-data rows or acts', () => {
  show(<HostKept host={host(['host.view'])} />);
  expect(screen.queryByText('Empty kept workspace')).not.toBeInTheDocument();
  expect(screen.queryByRole('heading', { name: 'Kept after removal' })).not.toBeInTheDocument();
  expect(read).not.toHaveBeenCalled();
});

it('QS-013 an omitted optional kept_data list is the served empty state', () => {
  const { access } = host();
  show(<HostKept host={{ access }} />);
  expect(screen.getByText('No environments have files kept here')).toBeInTheDocument();
  expect(within(screen.getByRole('region', { name: 'Kept after removal' })).queryByRole('button')).not.toBeInTheDocument();
  expect(read).not.toHaveBeenCalled();
});
