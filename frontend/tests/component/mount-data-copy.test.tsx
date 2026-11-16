/** Copy data is a separate reviewed job, with the existing destination choices and Notify control. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CopyMountData } from '@/features/resources/mounts/Copy';
import { ApiError } from '@/api/client';
import type { Job, Mount, MountCopyCreateBody, MountCopyPlan, MountCopyPreflightBody } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/chat-rows';
import { detailMachine, detailMount } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  plan: vi.fn<(id: string, body: MountCopyPreflightBody) => Promise<MountCopyPlan>>(),
  start: vi.fn<(id: string, body: MountCopyCreateBody, key: string) => Promise<Job>>(),
  latest: vi.fn<() => Promise<Job | null>>(),
  job: vi.fn<() => Promise<Job>>(),
}));
vi.mock('@/api/resource-tools', async (original) => ({ ...await original<object>(),
  planMountCopy: calls.plan, startMountCopy: calls.start,
  mountCopyJobKey: (id: string) => ['mount-copy-job', id],
  mountCopyJobQuery: (id: string) => ({ queryKey: ['mount-copy-job', id], queryFn: calls.latest }),
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), machinesQuery: { queryKey: ['copy-machines'], queryFn: () => Promise.resolve([source, destination]) } }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: calls.job }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const now = '2026-11-16T00:00:00Z';
const source = { ...detailMachine, id: 'source', name: 'Source machine' };
const destination = { ...detailMachine, id: 'destination', name: 'Destination machine' };
const mount: Mount = { ...detailMount, locations: [{
  machine: source, host: null, kind: 'path', source: '/srv/data', storage: null, data_kind: null,
  can_remove: true, created_by: ADA, created_at: now,
}] };
function plan(into: MountCopyPreflightBody['into'] = null): MountCopyPlan {
  return { preflight_id: `review-${into ?? 'choice'}`, expires_at: '2027-11-16T00:00:00Z',
    mount, source, target: destination,
    copy: { measured: true, file_count: 2, total_bytes: 4096,
      location: { made: into === 'new_location', kind: 'volume', source: 'orch-mount-data', storage: null },
      holds: { measured: true, file_count: 1, total_bytes: 1024 }, into, replace: true },
    blockers: into === null ? [{ code: 'mount_copy_into_required', message: 'Choose the destination data fate', missing: null }] : [],
    notification: { preview_id: 'copy-recipients', people: 2, default_on: true },
  };
}
function job(state: Job['state'] = 'running'): Job {
  return { id: 'copy-job', kind: 'mount_copy', state, node_id: null, environment_id: null, provider_id: null, batch_id: null,
    created_at: now, created_by: ADA, started_at: now, finished_at: state === 'running' ? null : now, error: null, log: null,
    progress: { phase: 'copy', percent: 25, phases: [], files_done: 1, files_total: 2, bytes_done: 1024, bytes_total: 4096, eta_seconds: null, rate_bps: null, message: null }, result: null };
}
beforeEach(() => {
  vi.resetAllMocks();
  calls.latest.mockResolvedValue(null);
  calls.plan.mockImplementation((_id, body) => Promise.resolve(plan(body.into)));
  calls.job.mockResolvedValue(job());
  calls.start.mockImplementation(() => new Promise(() => undefined));
});
afterEach(async () => { cleanup(); await setLocale('en'); });

async function chooseDestination(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('combobox', { name: i18n.t('mountCopy.target') }));
  await user.click(await screen.findByRole('option', { name: /Destination machine/ }));
}

for (const locale of ['en', 'zh-CN'] as const) it(`copies only the reviewed destination once and keeps an explicit Notify choice in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<CopyMountData mount={mount} sourceMachine={source.id} onClose={vi.fn()} />);
  expect(screen.getByRole('button', { name: i18n.t('storage.guide') })).toHaveAttribute('data-help-heading', 'copy-mount-data');
  await chooseDestination(user);
  const choice = await screen.findByRole('combobox', { name: i18n.t('mountPlan.fates.into', { mount: mount.name }) });
  const submit = screen.getByRole('button', { name: i18n.t('mountCopy.copy') });
  expect(submit).toBeDisabled();
  await user.click(choice);
  await user.click(await screen.findByRole('option', { name: i18n.t('mountPlan.fates.keepBoth') }));
  await waitFor(() => expect(submit).toBeEnabled());
  expect(calls.plan.mock.calls.at(-1)).toEqual([mount.id, { source_machine_id: source.id, target_machine_id: destination.id, into: 'new_location' }]);
  const notify = screen.getByRole('checkbox', { name: i18n.t('operation.notify', { count: 2, number: '2' }) });
  await user.click(notify);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  await user.click(submit);
  await user.click(submit);
  await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
  expect(calls.start.mock.calls[0]?.slice(0, 2)).toEqual([mount.id, { preflight_id: 'review-new_location', notify: false }]);
  expect(submit).toHaveAttribute('aria-disabled', 'true');
});

it('keeps Replace unavailable for live or borrowed destination data while offering a new location', async () => {
  calls.plan.mockImplementation((_id, body) => Promise.resolve({ ...plan(body.into), copy: { ...plan(body.into).copy!, replace: false } }));
  const user = userEvent.setup();
  show(<CopyMountData mount={mount} sourceMachine={source.id} onClose={vi.fn()} />);
  await chooseDestination(user);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('mountPlan.fates.into', { mount: mount.name }) }));
  expect(await screen.findByRole('option', { name: i18n.t('mountPlan.fates.replace') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('option', { name: i18n.t('mountPlan.fates.keepBoth') })).not.toHaveAttribute('aria-disabled', 'true');
});

it('reopens the retained job without accepting another copy', async () => {
  calls.latest.mockResolvedValue(job());
  const close = vi.fn();
  show(<CopyMountData mount={mount} onClose={close} />);
  expect(await screen.findByRole('heading', { name: i18n.t('mountCopy.progress') })).toBeVisible();
  expect(calls.plan).not.toHaveBeenCalled();
  expect(calls.start).not.toHaveBeenCalled();
  await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('change.progress.hide') }));
  expect(close).toHaveBeenCalledOnce();
  expect(calls.start).not.toHaveBeenCalled();
});

it('keeps the copy draft and Notify choice after a changed source and uses a new key for its new review', async () => {
  let revision = 0;
  calls.plan.mockImplementation((_id, body) => Promise.resolve({ ...plan(body.into), preflight_id: `review-${++revision}` }));
  calls.start.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Changed', detail: 'The original location changed', request_id: 'changed-copy' }, null)).mockResolvedValueOnce(job());
  const user = userEvent.setup();
  show(<CopyMountData mount={mount} sourceMachine={source.id} onClose={vi.fn()} />);
  await chooseDestination(user);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('mountPlan.fates.into', { mount: mount.name }) }));
  await user.click(await screen.findByRole('option', { name: i18n.t('mountPlan.fates.keepBoth') }));
  const submit = screen.getByRole('button', { name: i18n.t('mountCopy.copy') });
  await waitFor(() => expect(submit).toBeEnabled());
  await user.click(screen.getByRole('checkbox', { name: i18n.t('operation.notify', { count: 2, number: '2' }) }));
  await user.click(submit);
  await user.click(await screen.findByRole('button', { name: i18n.t('liveWork.retry') }));
  await waitFor(() => expect(submit).toBeEnabled());
  expect(screen.getByRole('combobox', { name: i18n.t('mountCopy.source') })).toHaveValue('Source machine');
  expect(screen.getByRole('combobox', { name: i18n.t('mountCopy.target') })).toHaveValue('Destination machine');
  expect(screen.getByRole('checkbox', { name: i18n.t('operation.notify', { count: 2, number: '2' }) })).not.toBeChecked();
  await user.click(submit);
  await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(2));
  expect(calls.start.mock.calls[1]?.[1].preflight_id).not.toBe(calls.start.mock.calls[0]?.[1].preflight_id);
  expect(calls.start.mock.calls[1]?.[2]).not.toBe(calls.start.mock.calls[0]?.[2]);
});
