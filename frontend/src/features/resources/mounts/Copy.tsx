/** One reviewed copy act, shared by a mount's page, locations, machine list and Delete plan. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { machinesQuery } from '@/api/resources';
import { ApiError } from '@/api/client';
import { mountCopyJobKey, mountCopyJobQuery, mountKey, mountQuery, planMountCopy, startMountCopy } from '@/api/resource-tools';
import { cancelJob, forkKeys, jobQuery } from '@/api/forks';
import { Blockers } from '@/features/change/Blockers';
import { CopyDestination } from '@/features/change/CopyDestination';
import { Frame } from '@/features/change/Frame';
import { MeasurementReason, Measurements, MeasurementSize } from '@/features/change/Measurements';
import { committed, JobProgress } from '@/features/change/Progress';
import { blockerWords } from '@/features/change/plan';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import type { CopyInto, Job, Mount, PlanCopyHolds, PlanMountCopy } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';

const over = (job: Job) => job.state === 'succeeded' || job.state === 'failed' || job.state === 'cancelled' || job.state === 'lost';

function Size({ data }: { data: PlanMountCopy | PlanCopyHolds }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (data.measurement !== undefined && data.measurement !== null) return <MeasurementSize size={{ reference: data.measurement, measured: data.measured, file_count: data.file_count, total_bytes: data.total_bytes }} />;
  return <span className="text-sm text-text-muted tabular-nums">{data.measured ? `${t('liveWork.files', { count: data.file_count, value: formatNumber(data.file_count, locale) })} · ${formatBytes(data.total_bytes, locale)}` : '–'}</span>;
}

export function CopyMountData({ mount, sourceMachine, onClose }: { mount: Mount; sourceMachine?: string; onClose: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const locations = mount.locations.filter((location) => location.can_remove && location.source !== null);
  const [source, setSource] = useState(sourceMachine ?? (locations.length === 1 ? locations[0]?.machine.id ?? '' : ''));
  const [target, setTarget] = useState('');
  const [into, setInto] = useState<CopyInto | null>(null);
  const [notify, setNotify] = useState<boolean | undefined>();
  const [started, setStarted] = useState<Job | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const key = useRef({ review: '', value: crypto.randomUUID() });
  const finished = useRef<string | null>(null);
  const machines = useQuery(machinesQuery);
  const latest = useQuery({ ...mountCopyJobQuery(mount.id), refetchOnMount: 'always' });
  const initial = started ?? (reviewing ? null : latest.data ?? null);
  const progress = useQuery({ ...jobQuery(initial?.id ?? ''), enabled: initial !== null });
  const job = initial === null ? null : progress.data ?? initial;
  const targets = (machines.isError ? [] : machines.data ?? []).filter((machine) => machine.id !== source && (holds(machine.access, 'machine.manage') || holds(machine.access, 'host.manage')));
  const sourceReady = locations.some((location) => location.machine.id === source);
  const targetReady = targets.some((machine) => machine.id === target);
  const selected = sourceReady && targetReady;
  const planning = job === null && (reviewing || latest.isSuccess);
  const preview = useQuery({ queryKey: ['mount-copy-plan', mount.id, source, target, into],
    queryFn: () => planMountCopy(mount.id, { source_machine_id: source, target_machine_id: target, into }),
    enabled: planning && selected, refetchOnWindowFocus: false, refetchOnMount: 'always', retry: false });
  const plan = preview.data;
  const copy = plan?.copy;
  const measured = copy?.measured === true && (copy.holds === null || copy.holds.measured);
  const ready = selected && plan?.blockers.length === 0 && measured && !preview.isFetching && !preview.isError;
  const { refetch } = preview;
  const refresh = useCallback(() => { void refetch(); }, [refetch]);
  const start = useMutation({ mutationFn: () => {
    if (!ready) throw new Error(t('mountCopy.reviewRequired'));
    if (key.current.review !== plan.preflight_id) key.current = { review: plan.preflight_id, value: crypto.randomUUID() };
    return startMountCopy(mount.id, { preflight_id: plan.preflight_id, notify: notify ?? plan.notification.default_on }, key.current.value);
  }, onSuccess: (value) => {
    cache.setQueryData(mountCopyJobKey(mount.id), value);
    cache.setQueryData(forkKeys.job(value.id), value);
    setStarted(value);
  } });
  const stop = useMutation({ mutationFn: () => cancelJob(job?.id ?? ''), onSuccess: (value) => {
    cache.setQueryData(forkKeys.job(value.id), value);
    cache.setQueryData(mountCopyJobKey(mount.id), value);
    setCancelling(false);
  } });
  useEffect(() => {
    if (job === null || !over(job) || finished.current === job.id) return;
    finished.current = job.id;
    void cache.invalidateQueries({ queryKey: mountKey(mount.id) });
    void cache.invalidateQueries({ queryKey: ['mounts'] });
  }, [cache, job, mount.id]);
  const chooseSource = (value: string) => { setSource(value); if (target === value) setTarget(''); setInto(null); start.reset(); };
  const chooseTarget = (value: string) => { setTarget(value); setInto(null); start.reset(); };
  const choice = (value: PlanMountCopy) => value.holds === null ? null : <CopyDestination mount={mount.name} value={into ?? value.into} replace={value.replace} disabled={start.isPending} onChange={(value) => { setInto(value); start.reset(); }} />;
  const firstBlocker = plan?.blockers[0];
  const reason = locations.length === 0 ? t('mountCopy.noSource') : !sourceReady ? t('mountCopy.sourceRequired') : !targetReady ? t(targets.length === 0 && machines.isSuccess ? 'mountCopy.noTarget' : 'mountCopy.targetRequired') : firstBlocker === undefined ? null : blockerWords(firstBlocker, null, t).text;
  const ended = job !== null && over(job);
  return <Measurements plan={copy === undefined ? undefined : { mounts: [{ copy }] }} active={planning} onReady={refresh}>
    <Frame open steps={false} aside={<ArticleGuideLink article="storage-cleanup" heading="copy-mount-data" title={t('storage.guide')} />} title={t('mountCopy.title')} at={job === null ? 'plan' : ended ? 'done' : 'progress'} failed={job?.state === 'failed' || job?.state === 'lost'} onClose={onClose} dismissible={!start.isPending} footer={<>
      {job === null && <><NotifyPeople plan={plan?.notification} value={notify} onChange={setNotify} disabled={start.isPending || preview.isFetching} /><span className="mr-auto text-sm text-text-muted"><MeasurementReason waiting={selected && !measured} planning={preview.isFetching} fallback={reason} /></span></>}
      {job !== null && !ended && !committed(job) && <Button variant="danger-ghost" pending={stop.isPending} disabled={job.progress.activity?.stopping === true} onClick={() => setCancelling(true)}>{t(job.progress.activity?.stopping === true ? 'liveWork.stopping' : 'change.progress.cancel')}</Button>}
      <Button disabled={start.isPending} onClick={onClose}>{t(job === null ? 'common.cancel' : ended ? 'common.close' : 'change.progress.hide')}</Button>
      {ended && <Button onClick={() => { setReviewing(true); setStarted(null); key.current = { review: '', value: crypto.randomUUID() }; start.reset(); }}>{t('mountCopy.again')}</Button>}
      {job === null && <Button variant="primary" pending={start.isPending} disabled={!ready} onClick={() => start.mutate()}>{t('mountCopy.copy')}</Button>}
    </>}>
      {job !== null ? <>
        <JobProgress job={job} heading={t('mountCopy.progress')} reconnecting={progress.isError} />
        {progress.isError && <ErrorState compact error={progress.error} onRetry={() => void progress.refetch()} />}
        {ended && <StateLine title={t(`source.job.${job.state}`)} tone={job.state === 'failed' ? 'warning' : 'neutral'} message={job.error?.message} />}
        <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('mountCopy.cancelTitle')} confirmLabel={t('change.progress.cancel')} danger pending={stop.isPending} error={stop.error ?? undefined} onConfirm={() => stop.mutate()}>{t('mountCopy.cancelBody')}</ConfirmDialog>
      </> : latest.data === undefined && !reviewing ? latest.isError ? <ErrorState error={latest.error} onRetry={() => void latest.refetch()} /> : <SkeletonRows read={latest} rows={3} /> : <>
        <div className="flex flex-col gap-3">
          <ResourcePicker pickerKey="mount-copy-source" label={t('mountCopy.source')} value={source} disabled={start.isPending} onValueChange={chooseSource} options={locations.map((location) => ({ value: location.machine.id, name: location.host?.name ?? location.machine.name, resource: { kind: 'machine', id: location.machine.id } }))} />
          <ResourcePicker pickerKey="mount-copy-target" label={t('mountCopy.target')} value={target} disabled={start.isPending} loading={machines.isPending} onValueChange={chooseTarget} options={targets.map((machine) => ({ value: machine.id, name: machine.host?.name ?? machine.name, resource: { kind: 'machine', id: machine.id, data: machine } }))} />
        </div>
        {machines.isError && <ErrorState compact error={machines.error} onRetry={() => void machines.refetch()} />}
        {preview.isFetching && plan === undefined && selected && <SkeletonRows read={preview} rows={2} />}
        {preview.isError && <ErrorState compact error={preview.error} onRetry={refresh} />}
        {copy !== undefined && copy !== null && <Table label={t('mountCopy.plan')} boundedIdentity rows={[copy]} rowKey={() => mount.id} columns={[
          { id: 'name', header: t('resources.listName'), grow: true, cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1"><ResourceChip subject={{ kind: 'mount', mount: plan?.mount ?? mount }} hint={false} />{!shown.has('size') && <Size data={row} />}{!shown.has('holds') && row.holds !== null && <span className="flex flex-wrap gap-1 text-sm text-text-muted">{t('mountCopy.targetData')} · <Size data={row.holds} /></span>}{!shown.has('choice') && choice(row)}</div> },
          { id: 'size', header: t('mountCopy.sourceData'), from: 'sm', cell: (row) => <Size data={row} /> },
          { id: 'holds', header: t('mountCopy.targetData'), from: 'md', cell: (row) => row.holds === null ? '–' : <Size data={row.holds} /> },
          { id: 'choice', header: t('mountCopy.target'), from: 'sm', cell: choice },
        ]} />}
        <Blockers blockers={plan?.blockers ?? []} plan={null} />
        {start.error !== null && <ErrorAlert error={start.error} action={start.error instanceof ApiError && (start.error.is('conflict.stale_revision') || start.error.is('plan.blocked') || start.error.is('not_found')) ? <Button size="sm" onClick={() => { start.reset(); refresh(); }}>{t('liveWork.retry')}</Button> : undefined} />}
      </>}
    </Frame>
  </Measurements>;
}

/** Doors that carry only a mount reference use the same current reader and its local retry. */
export function CopyMountDataLoader({ mountId, sourceMachine, onClose }: { mountId: string; sourceMachine?: string; onClose: () => void }) {
  const { t } = useTranslation();
  const read = useQuery(mountQuery(mountId));
  if (read.data !== undefined && !read.isError) return <CopyMountData mount={read.data} sourceMachine={sourceMachine} onClose={onClose} />;
  return <Frame open steps={false} aside={<ArticleGuideLink article="storage-cleanup" heading="copy-mount-data" title={t('storage.guide')} />} title={t('mountCopy.title')} at="plan" onClose={onClose} footer={<Button onClick={onClose}>{t('common.cancel')}</Button>}>
    {read.isError ? <ErrorState error={read.error} onRetry={() => void read.refetch()} /> : <SkeletonRows read={read} rows={3} />}
  </Frame>;
}
